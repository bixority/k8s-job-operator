use kube::CustomResource;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

#[derive(CustomResource, Debug, Clone, Deserialize, Serialize, JsonSchema)]
#[kube(
    group = "lambda.example.com",
    version = "v1",
    kind = "Task",
    namespaced
)]
#[kube(status = "TaskStatus")]
#[serde(rename_all = "camelCase")]
pub struct TaskSpec {
    /// Container image to run
    pub image: String,

    /// Optional image pull policy
    #[serde(default = "default_pull_policy")]
    pub image_pull_policy: String,

    /// Resource requirements
    #[serde(default)]
    pub resources: TaskResources,

    /// Environment variables to pass to the container
    #[serde(default)]
    pub env: Vec<TaskEnvVar>,

    /// Handler function name (simulates Lambda handler)
    #[serde(default = "default_handler")]
    pub handler: String,

    /// Timeout in seconds
    #[serde(default = "default_timeout")]
    pub timeout: i64,
}

fn default_pull_policy() -> String {
    "IfNotPresent".to_string()
}

fn default_handler() -> String {
    "handler".to_string()
}

fn default_timeout() -> i64 {
    300
}

#[derive(Debug, Clone, Deserialize, Serialize, JsonSchema, Default)]
#[serde(rename_all = "camelCase")]
pub struct TaskResources {
    #[serde(default)]
    pub limits: ResourceList,
    #[serde(default)]
    pub requests: ResourceList,
}

#[derive(Debug, Clone, Deserialize, Serialize, JsonSchema, Default)]
pub struct ResourceList {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cpu: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub memory: Option<String>,
}

#[derive(Debug, Clone, Deserialize, Serialize, JsonSchema)]
pub struct TaskEnvVar {
    pub name: String,
    pub value: String,
}

#[derive(Debug, Clone, Deserialize, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct TaskStatus {
    pub executions: i64,
    pub last_execution: Option<String>,
}

// HTTP API request/response types
#[derive(Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct InvokeRequest {
    pub kwargs: serde_json::Value,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub request_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub async_mode: Option<bool>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct InvokeResponse {
    pub request_id: String,
    pub job_name: String,
    pub status: String,
    pub namespace: String,
    pub task_name: String,
}

#[derive(Debug, Serialize)]
pub struct ErrorResponse {
    pub error: String,
    pub details: Option<String>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct HealthResponse {
    pub status: String,
    pub version: String,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TaskListResponse {
    pub tasks: Vec<TaskInfo>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TaskInfo {
    pub name: String,
    pub namespace: String,
    pub image: String,
    pub handler: String,
}
