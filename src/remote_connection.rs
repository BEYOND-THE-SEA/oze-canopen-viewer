//! Remote connection module for connecting to CAN interfaces over the network.
//!
//! This module provides functionality to:
//! - Connect to a remote Linux machine via SSH
//! - Deploy and manage cannelloni on the remote machine
//! - Create and manage local virtual CAN interfaces (vcan0)
//! - Bridge remote CAN traffic to the local virtual interface

use std::path::PathBuf;
use std::env;
use std::process::Stdio;
use std::sync::Mutex;
use tokio::process::Command;
use tokio::io::AsyncWriteExt;

/// Global storage for cleanup info (used by signal handlers)
static CLEANUP_INFO: Mutex<Option<CleanupInfo>> = Mutex::new(None);

/// Information needed to cleanup remote connection
#[derive(Clone, Debug)]
pub struct CleanupInfo {
    pub ssh_host: String,
    pub ssh_user: String,
    pub ssh_password: String,
    pub port: u16,
}

impl CleanupInfo {
    /// Store cleanup info for signal handlers
    pub fn store(info: CleanupInfo) {
        if let Ok(mut guard) = CLEANUP_INFO.lock() {
            *guard = Some(info);
        }
    }
    
    /// Clear stored cleanup info
    pub fn clear() {
        if let Ok(mut guard) = CLEANUP_INFO.lock() {
            *guard = None;
        }
    }
    
    /// Get stored cleanup info
    pub fn get() -> Option<CleanupInfo> {
        CLEANUP_INFO.lock().ok().and_then(|guard| guard.clone())
    }
    
    /// Perform cleanup (stop remote server and local client)
    pub fn cleanup_sync() {
        if let Some(info) = Self::get() {
            log::info!("Signal handler: cleaning up remote connection to {}", info.ssh_host);
            
            // Use std::process::Command for sync context (signal handler)
            // Stop local cannelloni client
            let _ = std::process::Command::new("pkill")
                .arg("-f")
                .arg(format!("cannelloni.*-R {}.*-r {}", info.ssh_host, info.port))
                .output();
            
            // Stop remote cannelloni server via SSH
            let _ = std::process::Command::new("sshpass")
                .arg("-p")
                .arg(&info.ssh_password)
                .arg("ssh")
                .arg("-o").arg("StrictHostKeyChecking=no")
                .arg("-o").arg("UserKnownHostsFile=/dev/null")
                .arg("-o").arg("ConnectTimeout=5")
                .arg(format!("{}@{}", info.ssh_user, info.ssh_host))
                .arg(format!("sudo pkill -f 'cannelloni.*-l {}' 2>/dev/null || true", info.port))
                .output();
            
            Self::clear();
            log::info!("Signal handler: cleanup completed");
        }
    }
}

/// Default port for cannelloni TCP communication
pub const DEFAULT_CANNELLONI_PORT: u16 = 29536;

/// Default timeout for remote cannelloni server (auto-cleanup)
pub const DEFAULT_SERVER_TIMEOUT: &str = "4h";

/// Remote connection configuration and operations
#[derive(Clone, Debug)]
pub struct RemoteConnection {
    pub ssh_host: String,
    pub ssh_user: String,
    pub ssh_password: String,
    pub remote_can_interface: String,
    pub remote_bitrate: Option<u32>,
    pub port: u16,
}

impl Default for RemoteConnection {
    fn default() -> Self {
        Self {
            ssh_host: String::new(),
            ssh_user: String::new(),
            ssh_password: String::new(),
            remote_can_interface: "can0".to_string(),
            remote_bitrate: Some(250_000),
            port: DEFAULT_CANNELLONI_PORT,
        }
    }
}

impl RemoteConnection {
    pub fn new(
        ssh_host: String,
        ssh_user: String,
        ssh_password: String,
        remote_can_interface: String,
        remote_bitrate: Option<u32>,
        port: u16,
    ) -> Self {
        Self {
            ssh_host,
            ssh_user,
            ssh_password,
            remote_can_interface,
            remote_bitrate,
            port,
        }
    }

    /// Get the path to the bundled cannelloni binary
    pub fn get_cannelloni_binary_path() -> PathBuf {
        // 1. Check in bin/ relative to the executable
        if let Ok(exe_path) = env::current_exe() {
            if let Some(exe_dir) = exe_path.parent() {
                // In bin/ next to the executable
                let bin_path = exe_dir.join("bin").join("cannelloni");
                if bin_path.exists() {
                    return bin_path;
                }

                // In the same directory as the executable
                let same_dir = exe_dir.join("cannelloni");
                if same_dir.exists() {
                    return same_dir;
                }
            }
        }

        // 2. Check in the project directory (for development)
        let project_bin = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("bin").join("cannelloni");
        if project_bin.exists() {
            return project_bin;
        }

        // 3. Fallback: check in system path
        PathBuf::from("/usr/local/bin/cannelloni")
    }

    /// Check if cannelloni is installed on the remote machine
    pub async fn check_cannelloni_installed(&self) -> Result<bool, String> {
        let output = self.execute_ssh_command("which cannelloni 2>/dev/null || echo ''").await?;
        Ok(!output.trim().is_empty() && output.contains("cannelloni"))
    }

    /// Deploy cannelloni binary to the remote machine
    pub async fn deploy_cannelloni(&self) -> Result<(), String> {
        let local_binary = Self::get_cannelloni_binary_path();

        if !local_binary.exists() {
            return Err(format!(
                "Cannelloni binary not found at: {}. Please ensure bin/cannelloni exists.",
                local_binary.display()
            ));
        }

        log::info!(
            "Deploying cannelloni from {} to {}@{}",
            local_binary.display(),
            self.ssh_user,
            self.ssh_host
        );

        // Copy via SCP using sshpass
        let scp_output = Command::new("sshpass")
            .arg("-p")
            .arg(&self.ssh_password)
            .arg("scp")
            .arg("-o")
            .arg("StrictHostKeyChecking=no")
            .arg("-o")
            .arg("UserKnownHostsFile=/dev/null")
            .arg(local_binary.to_str().unwrap())
            .arg(format!(
                "{}@{}:/tmp/cannelloni",
                self.ssh_user, self.ssh_host
            ))
            .stderr(Stdio::piped())
            .stdout(Stdio::piped())
            .output()
            .await
            .map_err(|e| {
                format!(
                    "Failed to execute scp. Is sshpass installed? (sudo apt install sshpass): {}",
                    e
                )
            })?;

        if !scp_output.status.success() {
            let error = String::from_utf8_lossy(&scp_output.stderr);
            return Err(format!("Failed to copy cannelloni binary: {}", error));
        }

        // Install on remote machine
        self.execute_ssh_command(
            "sudo mv /tmp/cannelloni /usr/local/bin/ && sudo chmod +x /usr/local/bin/cannelloni",
        )
        .await?;

        log::info!("Cannelloni deployed successfully to remote host");
        Ok(())
    }

    /// Execute a command on the remote machine via SSH
    pub async fn execute_ssh_command(&self, command: &str) -> Result<String, String> {
        // If command contains sudo, we need to pass the password via stdin
        let actual_command = if command.contains("sudo") {
            // Replace sudo with echo password | sudo -S to pass password via stdin
            command.replace("sudo ", &format!("echo '{}' | sudo -S ", self.ssh_password))
        } else {
            command.to_string()
        };

        let output = Command::new("sshpass")
            .arg("-p")
            .arg(&self.ssh_password)
            .arg("ssh")
            .arg("-o")
            .arg("StrictHostKeyChecking=no")
            .arg("-o")
            .arg("UserKnownHostsFile=/dev/null")
            .arg("-o")
            .arg("ConnectTimeout=10")
            .arg("-t")  // Allocate pseudo-terminal
            .arg("-t")  // Force TTY allocation even without local tty
            .arg(format!("{}@{}", self.ssh_user, self.ssh_host))
            .arg(&actual_command)
            .stderr(Stdio::piped())
            .stdout(Stdio::piped())
            .output()
            .await
            .map_err(|e| {
                format!(
                    "Failed to execute SSH command. Is sshpass installed? (sudo apt install sshpass): {}",
                    e
                )
            })?;

        if !output.status.success() {
            let error = String::from_utf8_lossy(&output.stderr);
            // Filter out common non-error messages
            let filtered: String = error
                .lines()
                .filter(|line| {
                    !line.contains("Warning: Permanently added")
                        && !line.contains("Connection to")
                        && !line.contains("Pseudo-terminal")
                        && !line.contains("[sudo]")
                        && !line.trim().is_empty()
                })
                .collect::<Vec<_>>()
                .join("\n");
            
            if !filtered.is_empty() {
                return Err(format!("SSH command failed: {}", filtered));
            }
        }

        Ok(String::from_utf8_lossy(&output.stdout).to_string())
    }

    /// Test SSH connection to the remote machine
    pub async fn test_connection(&self) -> Result<(), String> {
        log::info!("Testing SSH connection to {}@{}", self.ssh_user, self.ssh_host);
        self.execute_ssh_command("echo 'Connection successful'").await?;
        log::info!("SSH connection test successful");
        Ok(())
    }

    /// Configure the CAN interface on the remote machine
    pub async fn setup_can_interface(&self) -> Result<(), String> {
        log::info!(
            "Setting up CAN interface {} on remote host with bitrate {:?}",
            self.remote_can_interface,
            self.remote_bitrate
        );

        // Bring down the interface (ignore errors if already down)
        let _ = self
            .execute_ssh_command(&format!(
                "sudo ip link set {} down 2>/dev/null || true",
                self.remote_can_interface
            ))
            .await;

        // Configure bitrate if provided (interface must be DOWN)
        if let Some(br) = self.remote_bitrate {
            self.execute_ssh_command(&format!(
                "sudo ip link set {} type can bitrate {}",
                self.remote_can_interface, br
            ))
            .await
            .map_err(|e| format!("Failed to set bitrate: {}", e))?;
        }

        // Bring up the interface
        self.execute_ssh_command(&format!(
            "sudo ip link set {} up",
            self.remote_can_interface
        ))
        .await
        .map_err(|e| format!("Failed to bring interface up: {}", e))?;

        log::info!(
            "CAN interface {} configured successfully",
            self.remote_can_interface
        );
        Ok(())
    }

    /// Check if cannelloni server is already running
    pub async fn check_cannelloni_server_running(&self) -> Result<bool, String> {
        // Check if cannelloni process is running (simpler and more reliable)
        let output = self
            .execute_ssh_command("pgrep -f 'cannelloni.*-C s' || echo ''")
            .await?;
        Ok(!output.trim().is_empty() && output.trim() != "")
    }

    /// Stop cannelloni server on the remote machine
    pub async fn stop_cannelloni_server(&self) -> Result<(), String> {
        log::info!("Stopping cannelloni server on port {}", self.port);
        // Only kill the cannelloni server on our specific port
        let command = format!("sudo pkill -f 'cannelloni.*-l {}' 2>/dev/null || true", self.port);
        let _ = self
            .execute_ssh_command(&command)
            .await;
        // Wait a bit for the process to stop
        tokio::time::sleep(tokio::time::Duration::from_millis(300)).await;
        Ok(())
    }

    /// Start cannelloni server on the remote machine
    pub async fn start_cannelloni_server(&self) -> Result<(), String> {
        // Stop any existing server first
        self.stop_cannelloni_server().await?;

        log::info!(
            "Starting cannelloni server on {}:{} for interface {}",
            self.ssh_host,
            self.port,
            self.remote_can_interface
        );

        // Use bash -c with disown to properly background the process
        // timeout ensures auto-cleanup if app crashes without proper disconnect
        let command = format!(
            "sudo bash -c 'timeout {} cannelloni -I {} -C s -L 0.0.0.0 -l {} -p &' && sleep 1",
            DEFAULT_SERVER_TIMEOUT, self.remote_can_interface, self.port
        );

        self.execute_ssh_command(&command).await?;

        // Wait for server to start
        tokio::time::sleep(tokio::time::Duration::from_millis(1000)).await;

        // Verify server is running
        if self.check_cannelloni_server_running().await? {
            log::info!("Cannelloni server started successfully");
            Ok(())
        } else {
            // Try alternative method with systemd-run if available
            log::warn!("First method failed, trying alternative...");
            let alt_command = format!(
                "sudo systemd-run --scope timeout {} cannelloni -I {} -C s -L 0.0.0.0 -l {} -p 2>/dev/null || sudo bash -c 'nohup timeout {} cannelloni -I {} -C s -L 0.0.0.0 -l {} -p > /tmp/cannelloni.log 2>&1 &'",
                DEFAULT_SERVER_TIMEOUT, self.remote_can_interface, self.port,
                DEFAULT_SERVER_TIMEOUT, self.remote_can_interface, self.port
            );
            self.execute_ssh_command(&alt_command).await?;
            
            tokio::time::sleep(tokio::time::Duration::from_millis(1000)).await;
            
            if self.check_cannelloni_server_running().await? {
                log::info!("Cannelloni server started successfully (alternative method)");
                Ok(())
            } else {
                Err("Failed to start cannelloni server - process not running. Check if cannelloni is installed on remote.".to_string())
            }
        }
    }

    /// Complete setup of the remote machine with status callback
    pub async fn setup_remote_with_status<F>(&self, mut on_status: F) -> Result<(), String>
    where
        F: FnMut(RemoteSetupStatus),
    {
        // 1. Test connection
        on_status(RemoteSetupStatus::TestingConnection);
        self.test_connection().await?;

        // 2. Check/install cannelloni
        on_status(RemoteSetupStatus::CheckingCannelloni);
        if !self.check_cannelloni_installed().await? {
            on_status(RemoteSetupStatus::DeployingCannelloni);
            log::info!("Cannelloni not found on remote host, deploying...");
            self.deploy_cannelloni().await?;
        } else {
            log::info!("Cannelloni already installed on remote host");
        }

        // 3. Configure CAN interface
        on_status(RemoteSetupStatus::ConfiguringCanInterface);
        self.setup_can_interface().await?;

        // 4. Start cannelloni server
        on_status(RemoteSetupStatus::StartingServer);
        self.start_cannelloni_server().await?;

        Ok(())
    }

    /// Complete setup of the remote machine (without status callback)
    pub async fn setup_remote(&self) -> Result<(), String> {
        self.setup_remote_with_status(|_| {}).await
    }
}

/// Run a local command with sudo, passing password via stdin
async fn run_local_sudo(password: &str, args: &[&str]) -> Result<(), String> {
    let mut child = Command::new("sudo")
        .arg("-S")
        .args(args)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| format!("Failed to spawn sudo: {}", e))?;

    if let Some(mut stdin) = child.stdin.take() {
        stdin
            .write_all(format!("{password}\n").as_bytes())
            .await
            .map_err(|e| format!("Failed to write password: {}", e))?;
    }

    let output = child
        .wait_with_output()
        .await
        .map_err(|e| format!("Failed to wait for sudo: {}", e))?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        let filtered: String = stderr
            .lines()
            .filter(|line| !line.contains("[sudo]") && !line.contains("password"))
            .collect::<Vec<_>>()
            .join("\n");

        if !filtered.trim().is_empty() {
            return Err(filtered);
        }
    }

    Ok(())
}

/// Local cannelloni client management
pub struct LocalCannelloniClient;

impl LocalCannelloniClient {
    /// Create the virtual CAN interface (vcan0) using sudo with password
    pub async fn create_vcan_interface_with_password(password: &str) -> Result<(), String> {
        log::info!("Creating virtual CAN interface vcan0");

        // Load vcan module
        let _ = run_local_sudo(password, &["modprobe", "vcan"]).await;

        // Check if vcan0 already exists
        let check = Command::new("ip")
            .arg("link")
            .arg("show")
            .arg("vcan0")
            .stderr(Stdio::piped())
            .stdout(Stdio::piped())
            .output()
            .await;

        let vcan_exists = check.map(|o| o.status.success()).unwrap_or(false);

        if !vcan_exists {
            match run_local_sudo(
                password,
                &["ip", "link", "add", "dev", "vcan0", "type", "vcan"],
            )
            .await
            {
                Ok(()) => {}
                Err(e) => {
                    if !e.contains("File exists") {
                        return Err(format!("Failed to create vcan0: {}", e));
                    }
                }
            };
        }

        // Bring up the interface
        run_local_sudo(password, &["ip", "link", "set", "up", "vcan0"])
            .await
            .map_err(|e| format!("Failed to bring vcan0 up: {}", e))?;

        log::info!("Virtual CAN interface vcan0 created and up");
        Ok(())
    }

    /// Create the virtual CAN interface (vcan0) without explicit password (may fail if sudo requires it)
    pub async fn create_vcan_interface() -> Result<(), String> {
        Self::create_vcan_interface_with_password("").await
    }

    /// Check if cannelloni client is already running for the specified remote
    pub async fn check_client_running(remote_host: &str) -> Result<bool, String> {
        let output = Command::new("pgrep")
            .arg("-f")
            .arg(&format!("cannelloni.*vcan0.*{}", remote_host))
            .stderr(Stdio::piped())
            .stdout(Stdio::piped())
            .output()
            .await
            .map_err(|e| format!("Failed to check client: {}", e))?;

        Ok(output.status.success())
    }

    /// Stop cannelloni client using sudo with password
    pub async fn stop_client_with_password(password: &str) -> Result<(), String> {
        log::info!("Stopping local cannelloni client");
        let _ = run_local_sudo(password, &["pkill", "-f", "cannelloni.*vcan0"]).await;
        // Wait for process to stop
        tokio::time::sleep(tokio::time::Duration::from_millis(200)).await;
        Ok(())
    }

    /// Stop cannelloni client without explicit password (may fail if sudo requires it)
    pub async fn stop_client() -> Result<(), String> {
        Self::stop_client_with_password("").await
    }

    /// Start cannelloni client connecting to remote server
    pub async fn start_client_with_password(
        remote_host: &str,
        port: u16,
        password: &str,
    ) -> Result<(), String> {
        // Stop any existing client first
        Self::stop_client_with_password(password).await?;

        let cannelloni_path = RemoteConnection::get_cannelloni_binary_path();

        if !cannelloni_path.exists() {
            return Err(format!(
                "Cannelloni binary not found at: {}. Please ensure bin/cannelloni exists.",
                cannelloni_path.display()
            ));
        }

        log::info!(
            "Starting cannelloni client connecting to {}:{}",
            remote_host,
            port
        );

        let cmd = format!(
            "{} -I vcan0 -C c -R {} -r {}",
            cannelloni_path.to_str().unwrap(),
            remote_host,
            port
        );
        let bash_cmd = format!("{cmd} &");

        run_local_sudo(password, &["bash", "-c", &bash_cmd])
            .await
            .map_err(|e| format!("Failed to start cannelloni client: {}", e))?;

        // Wait for client to connect
        tokio::time::sleep(tokio::time::Duration::from_millis(500)).await;

        if Self::check_client_running(remote_host).await? {
            log::info!("Cannelloni client started successfully");
            Ok(())
        } else {
            Err("Cannelloni client failed to start or connect".to_string())
        }
    }

    /// Start cannelloni client connecting to remote server without explicit password
    pub async fn start_client(remote_host: &str, port: u16) -> Result<(), String> {
        Self::start_client_with_password(remote_host, port, "").await
    }

    /// Complete local setup: create vcan0 and start client with status callback
    pub async fn setup_local_with_status<F>(remote_host: &str, port: u16, mut on_status: F) -> Result<(), String>
    where
        F: FnMut(RemoteSetupStatus),
    {
        on_status(RemoteSetupStatus::CreatingVcan);
        Self::create_vcan_interface().await?;
        
        on_status(RemoteSetupStatus::StartingClient);
        Self::start_client(remote_host, port).await?;
        
        Ok(())
    }

    /// Complete local setup: create vcan0 and start client
    pub async fn setup_local(remote_host: &str, port: u16) -> Result<(), String> {
        Self::setup_local_with_status(remote_host, port, |_| {}).await
    }

    /// Stop all local cannelloni processes and optionally remove vcan0 using sudo with password
    pub async fn cleanup_with_password(remove_vcan: bool, password: &str) -> Result<(), String> {
        Self::stop_client_with_password(password).await?;

        if remove_vcan {
            log::info!("Removing vcan0 interface");
            let _ = run_local_sudo(password, &["ip", "link", "delete", "vcan0"]).await;
        }

        Ok(())
    }

    /// Stop all local cannelloni processes and optionally remove vcan0 without explicit password
    pub async fn cleanup(remove_vcan: bool) -> Result<(), String> {
        Self::cleanup_with_password(remove_vcan, "").await
    }
}

/// Status of the remote connection setup
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum RemoteSetupStatus {
    Idle,
    TestingConnection,
    CheckingCannelloni,
    DeployingCannelloni,
    ConfiguringCanInterface,
    StartingServer,
    CreatingVcan,
    StartingClient,
    Connected,
    Failed(String),
}

impl std::fmt::Display for RemoteSetupStatus {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Idle => write!(f, "Idle"),
            Self::TestingConnection => write!(f, "Testing SSH connection..."),
            Self::CheckingCannelloni => write!(f, "Checking cannelloni on remote..."),
            Self::DeployingCannelloni => write!(f, "Deploying cannelloni to remote..."),
            Self::ConfiguringCanInterface => write!(f, "Configuring CAN interface..."),
            Self::StartingServer => write!(f, "Starting cannelloni server..."),
            Self::CreatingVcan => write!(f, "Creating local vcan0 interface..."),
            Self::StartingClient => write!(f, "Starting cannelloni client..."),
            Self::Connected => write!(f, "Connected"),
            Self::Failed(msg) => write!(f, "Failed: {}", msg),
        }
    }
}
