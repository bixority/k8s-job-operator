mod types;
use crate::types::{
    ErrorResponse, HealthResponse, InvokeRequest, InvokeResponse, Task, TaskInfo, TaskListResponse,
};
use axum::{
    Json, Router,
    extract::{Path, State},
    http::StatusCode,
    response::{IntoResponse, Response},
    routing::{get, post},
};
use futures::StreamExt;
use k8s_openapi::api::batch::v1::{Job, JobSpec};
use k8s_openapi::api::core::v1::{
    Container, EnvVar, PodSpec, PodTemplateSpec, ResourceRequirements,
};
use k8s_openapi::apimachinery::pkg::api::resource::Quantity;
use kube::{
    Client, ResourceExt,
    api::{Api, ObjectMeta, PostParams},
    runtime::controller::{Action, Controller},
};
use std::collections::BTreeMap;
use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;
use thiserror::Error;
use tower_http::trace::TraceLayer;
use tracing::{error, info, warn};

#[derive(Error, Debug)]
pub enum OperatorError {
    #[error("Kubernetes error: {0}")]
    KubeError(#[from] kube::Error),

    #[error("Serialization error: {0}")]
    SerdeError(#[from] serde_json::Error),

    #[error("Task not found: {0}")]
    TaskNotFound(String),

    #[error("Configuration error: {0}")]
    ConfigError(String),

    #[error("Invalid request: {0}")]
    InvalidRequest(String),
}

impl IntoResponse for OperatorError {
    fn into_response(self) -> Response {
        let (status, message) = match self {
            Self::TaskNotFound(ref msg) => (StatusCode::NOT_FOUND, msg.clone()),
            Self::InvalidRequest(ref msg) => (StatusCode::BAD_REQUEST, msg.clone()),
            Self::ConfigError(ref msg) => (StatusCode::INTERNAL_SERVER_ERROR, msg.clone()),
            Self::KubeError(ref e) => (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()),
            Self::SerdeError(ref e) => (StatusCode::BAD_REQUEST, e.to_string()),
        };

        let body = Json(ErrorResponse {
            error: message.clone(),
            details: Some(self.to_string()),
        });

        (status, body).into_response()
    }
}

#[derive(Clone)]
struct OperatorConfig {
    http_port: u16,
    default_namespace: String,
}

impl OperatorConfig {
    fn from_env() -> Result<Self, OperatorError> {
        Ok(Self {
            http_port: std::env::var("HTTP_PORT")
                .unwrap_or_else(|_| "8080".to_string())
                .parse()
                .map_err(|_| OperatorError::ConfigError("Invalid HTTP_PORT".to_string()))?,
            default_namespace: std::env::var("NAMESPACE").unwrap_or_else(|_| "default".to_string()),
        })
    }
}

#[derive(Clone)]
struct AppState {
    client: Client,
    config: OperatorConfig,
}

async fn reconcile_task(task: Arc<Task>, _ctx: Arc<AppState>) -> Result<Action, OperatorError> {
    info!("Reconciling task: {}", task.name_any());
    Ok(Action::requeue(Duration::from_secs(300)))
}

fn error_policy(_task: Arc<Task>, error: &OperatorError, _ctx: Arc<AppState>) -> Action {
    error!("Reconciliation error: {:?}", error);
    Action::requeue(Duration::from_secs(60))
}

async fn create_job_for_task(
    client: &Client,
    task: &Task,
    request: &InvokeRequest,
    namespace: &str,
) -> Result<String, OperatorError> {
    let request_id = request
        .request_id
        .clone()
        .unwrap_or_else(|| uuid::Uuid::new_v4().to_string());

    let job_name = format!("{}-{}", task.name_any(), chrono::Utc::now().timestamp());

    info!("Creating job: {} for task: {}", job_name, task.name_any());

    // Build environment variables
    let mut env_vars = vec![
        EnvVar {
            name: "LAMBDA_HANDLER".to_string(),
            value: Some(task.spec.handler.clone()),
            ..Default::default()
        },
        EnvVar {
            name: "LAMBDA_TASK_NAME".to_string(),
            value: Some(task.name_any()),
            ..Default::default()
        },
        EnvVar {
            name: "LAMBDA_REQUEST_ID".to_string(),
            value: Some(request_id.clone()),
            ..Default::default()
        },
        EnvVar {
            name: "LAMBDA_KWARGS".to_string(),
            value: Some(serde_json::to_string(&request.kwargs)?),
            ..Default::default()
        },
    ];

    // Add custom environment variables from task spec
    for task_env in &task.spec.env {
        env_vars.push(EnvVar {
            name: task_env.name.clone(),
            value: Some(task_env.value.clone()),
            ..Default::default()
        });
    }

    // Build resource requirements
    let mut limits = BTreeMap::new();
    let mut requests = BTreeMap::new();

    if let Some(cpu) = &task.spec.resources.limits.cpu {
        limits.insert("cpu".to_string(), Quantity(cpu.clone()));
    }
    if let Some(memory) = &task.spec.resources.limits.memory {
        limits.insert("memory".to_string(), Quantity(memory.clone()));
    }
    if let Some(cpu) = &task.spec.resources.requests.cpu {
        requests.insert("cpu".to_string(), Quantity(cpu.clone()));
    }
    if let Some(memory) = &task.spec.resources.requests.memory {
        requests.insert("memory".to_string(), Quantity(memory.clone()));
    }

    let resources = if limits.is_empty() && requests.is_empty() {
        None
    } else {
        Some(ResourceRequirements {
            limits: if limits.is_empty() {
                None
            } else {
                Some(limits)
            },
            requests: if requests.is_empty() {
                None
            } else {
                Some(requests)
            },
            ..Default::default()
        })
    };

    // Create container
    let container = Container {
        name: "task".to_string(),
        image: Some(task.spec.image.clone()),
        image_pull_policy: Some(task.spec.image_pull_policy.clone()),
        env: Some(env_vars),
        resources,
        ..Default::default()
    };

    // Create Job
    let mut labels = BTreeMap::new();
    labels.insert("app".to_string(), "lambda-task".to_string());
    labels.insert("task".to_string(), task.name_any());
    labels.insert("request-id".to_string(), request_id.clone());

    let job = Job {
        metadata: ObjectMeta {
            name: Some(job_name.clone()),
            namespace: Some(namespace.to_string()),
            labels: Some(labels.clone()),
            ..Default::default()
        },
        spec: Some(JobSpec {
            template: PodTemplateSpec {
                metadata: Some(ObjectMeta {
                    labels: Some(labels),
                    ..Default::default()
                }),
                spec: Some(PodSpec {
                    containers: vec![container],
                    restart_policy: Some("Never".to_string()),
                    active_deadline_seconds: Some(task.spec.timeout),
                    ..Default::default()
                }),
            },
            backoff_limit: Some(0),
            ttl_seconds_after_finished: Some(3600),
            ..Default::default()
        }),
        ..Default::default()
    };

    let jobs: Api<Job> = Api::namespaced(client.clone(), namespace);
    jobs.create(&PostParams::default(), &job).await?;

    info!("Job created successfully: {}", job_name);
    Ok(job_name)
}

// HTTP Handlers

async fn health_check() -> Json<HealthResponse> {
    Json(HealthResponse {
        status: "healthy".to_string(),
        version: env!("CARGO_PKG_VERSION").to_string(),
    })
}

async fn list_tasks(
    State(state): State<Arc<AppState>>,
) -> Result<Json<TaskListResponse>, OperatorError> {
    let tasks: Api<Task> = Api::all(state.client.clone());
    let task_list = tasks.list(&Default::default()).await?;

    let task_infos: Vec<TaskInfo> = task_list
        .items
        .iter()
        .map(|task| TaskInfo {
            name: task.name_any(),
            namespace: task.namespace().unwrap_or_default(),
            image: task.spec.image.clone(),
            handler: task.spec.handler.clone(),
        })
        .collect();

    Ok(Json(TaskListResponse { tasks: task_infos }))
}

async fn get_task(
    State(state): State<Arc<AppState>>,
    Path((namespace, task_name)): Path<(String, String)>,
) -> Result<Json<Task>, OperatorError> {
    let tasks: Api<Task> = Api::namespaced(state.client.clone(), &namespace);
    let task = tasks.get(&task_name).await?;
    Ok(Json(task))
}

async fn invoke_task(
    State(state): State<Arc<AppState>>,
    Path((namespace, task_name)): Path<(String, String)>,
    Json(request): Json<InvokeRequest>,
) -> Result<Json<InvokeResponse>, OperatorError> {
    info!("Invoking task: {} in namespace: {}", task_name, namespace);

    // Get the task CRD
    let tasks: Api<Task> = Api::namespaced(state.client.clone(), &namespace);
    let task = tasks
        .get(&task_name)
        .await
        .map_err(|_| OperatorError::TaskNotFound(task_name.clone()))?;

    // Create job
    let job_name = create_job_for_task(&state.client, &task, &request, &namespace).await?;

    let request_id = request
        .request_id
        .unwrap_or_else(|| uuid::Uuid::new_v4().to_string());

    Ok(Json(InvokeResponse {
        request_id,
        job_name,
        status: "accepted".to_string(),
        namespace: namespace.clone(),
        task_name: task_name.clone(),
    }))
}

async fn invoke_task_default_namespace(
    State(state): State<Arc<AppState>>,
    Path(task_name): Path<String>,
    Json(request): Json<InvokeRequest>,
) -> Result<Json<InvokeResponse>, OperatorError> {
    let namespace = state.config.default_namespace.clone();
    invoke_task(State(state), Path((namespace, task_name)), Json(request)).await
}

fn create_router(state: Arc<AppState>) -> Router {
    Router::new()
        .route("/health", get(health_check))
        .route("/tasks", get(list_tasks))
        .route("/tasks/:namespace/:task_name", get(get_task))
        .route("/tasks/:namespace/:task_name/invoke", post(invoke_task))
        .route("/invoke/:task_name", post(invoke_task_default_namespace))
        .layer(TraceLayer::new_for_http())
        .with_state(state)
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            std::env::var("RUST_LOG").unwrap_or_else(|_| "info,kube=info,axum=info".to_string()),
        )
        .init();

    info!("Starting Lambda-like Kubernetes Operator (HTTP Version)");

    // Load configuration
    let config = OperatorConfig::from_env()?;
    info!("Configuration loaded: port={}", config.http_port);

    // Initialize Kubernetes client
    let client = Client::try_default().await?;
    info!("Kubernetes client initialized");

    let state = Arc::new(AppState {
        client: client.clone(),
        config: config.clone(),
    });

    // Start controller for Task CRD
    let tasks: Api<Task> = Api::all(client.clone());
    let controller = Controller::new(tasks, Default::default())
        .run(reconcile_task, error_policy, state.clone())
        .for_each(|_| futures::future::ready(()));

    // Create HTTP server
    let app = create_router(state);
    let addr = SocketAddr::from(([0, 0, 0, 0], config.http_port));
    info!("Starting HTTP server on {}", addr);

    let listener = tokio::net::TcpListener::bind(addr).await?;

    // Run both controller and HTTP server
    tokio::select! {
        _ = controller => {
            warn!("Controller stopped");
        }
        result = axum::serve(listener, app) => {
            if let Err(e) = result {
                error!("HTTP server error: {}", e);
            }
            warn!("HTTP server stopped");
        }
    }

    Ok(())
}
