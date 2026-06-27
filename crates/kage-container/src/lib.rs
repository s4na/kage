use std::{
    path::{Path, PathBuf},
    process::{Command, ExitStatus},
};

pub type Result<T> = std::result::Result<T, Box<dyn std::error::Error>>;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ContainerRuntime {
    Docker,
    Podman,
    AppleContainer,
}

impl ContainerRuntime {
    pub fn binary(self) -> &'static str {
        match self {
            Self::Docker => "docker",
            Self::Podman => "podman",
            Self::AppleContainer => "container",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ContainerRunSpec {
    pub runtime: ContainerRuntime,
    pub workspace: PathBuf,
    pub image: String,
    pub command: Vec<String>,
    pub mountpoint: String,
}

impl ContainerRunSpec {
    pub fn new(
        runtime: ContainerRuntime,
        workspace: impl Into<PathBuf>,
        image: impl Into<String>,
        command: Vec<String>,
    ) -> Self {
        Self {
            runtime,
            workspace: workspace.into(),
            image: image.into(),
            command,
            mountpoint: "/workspace".to_string(),
        }
    }

    pub fn argv(&self) -> Vec<String> {
        match self.runtime {
            ContainerRuntime::Docker | ContainerRuntime::Podman => self.oci_argv(),
            ContainerRuntime::AppleContainer => self.apple_container_argv(),
        }
    }

    fn oci_argv(&self) -> Vec<String> {
        let mut args = vec![
            "run".to_string(),
            "--rm".to_string(),
            "-v".to_string(),
            format!("{}:{}", self.workspace.display(), self.mountpoint),
            "-w".to_string(),
            self.mountpoint.clone(),
            self.image.clone(),
        ];
        args.extend(self.command.clone());
        args
    }

    fn apple_container_argv(&self) -> Vec<String> {
        let mut args = vec![
            "run".to_string(),
            "--rm".to_string(),
            "--volume".to_string(),
            format!("{}:{}", self.workspace.display(), self.mountpoint),
            "--workdir".to_string(),
            self.mountpoint.clone(),
            self.image.clone(),
        ];
        args.extend(self.command.clone());
        args
    }
}

pub fn run(spec: &ContainerRunSpec) -> Result<ExitStatus> {
    let argv = spec.argv();
    Ok(Command::new(spec.runtime.binary()).args(argv).status()?)
}

pub fn apple_container_requires_managed_linux_vm() -> &'static str {
    "Apple Container runs Linux containers in Apple Silicon optimized lightweight managed Linux VMs; kage mounts the prepared workspace into that VM and keeps the Rust control plane on the host."
}

pub fn ensure_workspace_mountable(path: &Path) -> Result<()> {
    if path.is_dir() {
        Ok(())
    } else {
        Err(format!("workspace mount path does not exist: {}", path.display()).into())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn apple_container_argv_mounts_workspace_into_linux_vm() {
        let spec = ContainerRunSpec::new(
            ContainerRuntime::AppleContainer,
            "/tmp/kage/ws/merged",
            "ubuntu:latest",
            vec!["bash".to_string(), "-lc".to_string(), "pwd".to_string()],
        );

        assert_eq!(
            spec.argv(),
            vec![
                "run",
                "--rm",
                "--volume",
                "/tmp/kage/ws/merged:/workspace",
                "--workdir",
                "/workspace",
                "ubuntu:latest",
                "bash",
                "-lc",
                "pwd",
            ]
        );
        assert_eq!(spec.runtime.binary(), "container");
    }

    #[test]
    fn docker_and_podman_use_standard_bind_mount_shape() {
        let spec = ContainerRunSpec::new(
            ContainerRuntime::Docker,
            "/tmp/ws",
            "rust:latest",
            vec!["cargo".to_string(), "test".to_string()],
        );
        assert_eq!(
            spec.argv()[..6],
            [
                "run",
                "--rm",
                "-v",
                "/tmp/ws:/workspace",
                "-w",
                "/workspace"
            ]
        );
    }
}
