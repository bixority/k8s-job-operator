use kube::CustomResourceExt;
use k8s_job_operator::types::Task;

fn main() {
    print!("{}", serde_yaml::to_string(&Task::crd()).unwrap());
}
