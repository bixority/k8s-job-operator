# Lambda Kubernetes Operator - HTTP Version

An HTTP-based version of the Lambda Kubernetes operator that receives task invocations via REST API.

## Overview

This operator provides the same Lambda-like functionality but exposes an HTTP API for task invocation, making it easier to integrate with web applications, microservices, and serverless frameworks.

## API Endpoints

### Health Check
```http
GET /health
```

Response:
```json
{
  "status": "healthy",
  "version": "0.1.0"
}
```

Response:
```json
{
  "tasks": [
    {
      "name": "image-processor",
      "namespace": "default",
      "image": "myregistry/image-processor:v1.0",
      "handler": "process_image"
    }
  ]
}
```

Response: Returns the complete Task CRD definition.

### Invoke Task (Full)
```http
POST /tasks/{namespace}/{task_name}/invoke
```

Request body:
```json
{
  "kwargs": {
    "key": "value",
    "data": [1, 2, 3]
  },
  "requestId": "optional-request-id",
  "asyncMode": true
}
```

Response:
```json
{
  "requestId": "req-abc-123",
  "jobName": "image-processor-1699876543",
  "status": "accepted",
  "namespace": "default",
  "taskName": "image-processor"
}
```

### Invoke Task (Simple)
```http
POST /invoke/{task_name}
```

Uses the default namespace specified in operator configuration.

Request body:
```json
{
  "kwargs": {
    "key": "value"
  }
}
```

## Prerequisites

- Kubernetes 1.34+
- Rust 1.91+ (2024 edition) for building
- Task CRD

## Installation

### Build

```bash
# Build HTTP operator
cargo build --release --bin k8s-job-operator

# Generate CRD
cargo run --bin crdgen > manifests/crd.yaml
```

### Docker Build

```dockerfile
FROM rust:1.91 as builder
WORKDIR /app
COPY Cargo.toml Cargo.lock ./
COPY src ./src
RUN cargo build --release --bin lambda-operator-http

FROM debian:bookworm-slim
RUN apt-get update && \
    apt-get install -y ca-certificates && \
    rm -rf /var/lib/apt/lists/*
COPY --from=builder /app/target/release/lambda-operator-http /usr/local/bin/
EXPOSE 8080
CMD ["lambda-operator-http"]
```

Build and push:
```bash
docker build -t your-registry/lambda-operator-http:latest -f Containerfile .
docker push your-registry/lambda-operator-http:latest
```

### Deploy to Kubernetes

1. **Install CRD**:
```bash
kubectl apply -f manifests/crd.yaml
```

2. **Deploy HTTP operator**:
```bash
kubectl apply -f manifests/deployment-http.yaml
```

3. **Verify deployment**:
```bash
kubectl get pods -l app=lambda-http-operator
kubectl logs -l app=lambda-http-operator -f
```

4. **Test internally**:
```bash
kubectl port-forward svc/lambda-http-operator 8080:80
curl http://localhost:8080/health
```

## Configuration

Environment variables:

| Variable | Description | Default |
|----------|-------------|---------|
| `HTTP_PORT` | Port for HTTP server | `8080` |
| `NAMESPACE` | Default namespace for tasks | `default` |
| `RUST_LOG` | Log level | `info` |

## Usage Examples

### Python Client

```python
import requests
import json

class LambdaHTTPClient:
    def __init__(self, base_url):
        self.base_url = base_url.rstrip('/')
    
    def invoke(self, task_name, kwargs, namespace="default"):
        response = requests.post(
            f"{self.base_url}/tasks/{namespace}/{task_name}/invoke",
            json={"kwargs": kwargs}
        )
        response.raise_for_status()
        return response.json()

# Usage
client = LambdaHTTPClient("http://lambda-operator.example.com")
result = client.invoke(
    task_name="image-processor",
    kwargs={
        "image_url": "s3://bucket/image.jpg",
        "filters": ["resize", "compress"]
    }
)
print(f"Job created: {result['jobName']}")
```

### Curl

```bash
# Simple invocation (uses default namespace)
curl -X POST http://lambda-operator.example.com/invoke/my-task \
  -H "Content-Type: application/json" \
  -d '{
    "kwargs": {
      "input": "test data"
    }
  }'

# Full invocation with namespace and request ID
curl -X POST http://lambda-operator.example.com/tasks/production/image-processor/invoke \
  -H "Content-Type: application/json" \
  -d '{
    "kwargs": {
      "image_url": "s3://bucket/image.jpg",
      "filters": ["resize", "compress"]
    },
    "requestId": "req-12345"
  }'

# List all tasks
curl http://lambda-operator.example.com/tasks

# Get task details
curl http://lambda-operator.example.com/tasks/default/my-task

# Health check
curl http://lambda-operator.example.com/health
```

### With FastAPI

```python
from fastapi import FastAPI, HTTPException, BackgroundTasks
import httpx
from typing import Dict, Any

app = FastAPI()
operator_url = "http://lambda-http-operator.default.svc"

class LambdaClient:
    def __init__(self, base_url: str):
        self.base_url = base_url
    
    async def invoke(self, task_name: str, kwargs: Dict[str, Any]):
        async with httpx.AsyncClient() as client:
            response = await client.post(
                f"{self.base_url}/invoke/{task_name}",
                json={"kwargs": kwargs},
                timeout=30.0
            )
            response.raise_for_status()
            return response.json()

lambda_client = LambdaClient(operator_url)

@app.post("/api/process-image")
async def process_image(image_url: str, filters: list[str]):
    """API endpoint that triggers image processing task."""
    try:
        result = await lambda_client.invoke(
            task_name="image-processor",
            kwargs={
                "image_url": image_url,
                "filters": filters
            }
        )
        return {
            "status": "accepted",
            "job_name": result["jobName"],
            "request_id": result["requestId"]
        }
    except httpx.HTTPStatusError as e:
        raise HTTPException(
            status_code=e.response.status_code,
            detail=str(e)
        )
```


## Monitoring


### ServiceMonitor for Prometheus Operator

```yaml
apiVersion: monitoring.coreos.com/v1
kind: ServiceMonitor
metadata:
  name: k8s-job-operator
  namespace: default
spec:
  selector:
    matchLabels:
      app: k8s-job-operator
  endpoints:
  - port: http
    path: /metrics
    interval: 30s
```

### Grafana Dashboard

Example queries:
```promql
# Request rate
rate(http_requests_total[5m])

# Success rate
sum(rate(http_requests_total{status=~"2.."}[5m])) /
sum(rate(http_requests_total[5m]))

# P99 latency
histogram_quantile(0.99, 
  rate(http_request_duration_seconds_bucket[5m])
)

# Jobs created per minute
rate(lambda_jobs_created_total[1m]) * 60
```

### Health Checks

Kubernetes probes configuration:

```yaml
livenessProbe:
  httpGet:
    path: /health
    port: http
  initialDelaySeconds: 10
  periodSeconds: 30
  timeoutSeconds: 5
  failureThreshold: 3

readinessProbe:
  httpGet:
    path: /ready
    port: http
  initialDelaySeconds: 5
  periodSeconds: 10
  timeoutSeconds: 3
  failureThreshold: 2
```

## License

AGPL-3.0

## Contributing

Contributions welcome! Please ensure:
- Code passes `cargo clippy -- -W clippy::pedantic`
- Tests pass with `cargo test`
- CRDs are regenerated if Task struct changes
- Documentation is updated
