use serde::{Deserialize, Serialize};
use std::path::PathBuf;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CiConfig {
    #[serde(default)]
    pub project: ProjectConfig,

    #[serde(default)]
    pub pipeline: PipelineConfig,

    #[serde(default)]
    pub watch: WatchConfig,

    #[serde(default)]
    pub dashboard: DashboardConfig,

    #[serde(default)]
    pub stages: Vec<StageDef>,
}

impl Default for CiConfig {
    fn default() -> Self {
        Self::default_pipeline()
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProjectConfig {
    #[serde(default = "default_name")]
    pub name: String,

    #[serde(default = "default_work_dir")]
    pub work_dir: PathBuf,

    #[serde(default = "default_dist_dir")]
    pub dist_dir: PathBuf,

    #[serde(default = "default_data_dir")]
    pub data_dir: PathBuf,

    #[serde(default)]
    pub env: std::collections::HashMap<String, String>,
}

impl Default for ProjectConfig {
    fn default() -> Self {
        Self {
            name: default_name(),
            work_dir: default_work_dir(),
            dist_dir: default_dist_dir(),
            data_dir: default_data_dir(),
            env: std::collections::HashMap::new(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PipelineConfig {
    #[serde(default = "default_max_parallel")]
    pub max_parallel: usize,

    #[serde(default = "default_timeout_secs")]
    pub timeout_secs: u64,

    #[serde(default = "default_true")]
    pub fail_fast: bool,

    #[serde(default = "default_branch_filter")]
    pub branch_filter: String,
}

impl Default for PipelineConfig {
    fn default() -> Self {
        Self {
            max_parallel: default_max_parallel(),
            timeout_secs: default_timeout_secs(),
            fail_fast: true,
            branch_filter: default_branch_filter(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WatchConfig {
    #[serde(default = "default_true")]
    pub enabled: bool,

    #[serde(default = "default_watch_paths")]
    pub paths: Vec<PathBuf>,

    #[serde(default = "default_ignore_patterns")]
    pub ignore_patterns: Vec<String>,

    #[serde(default = "default_debounce_ms")]
    pub debounce_ms: u64,
}

impl Default for WatchConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            paths: default_watch_paths(),
            ignore_patterns: default_ignore_patterns(),
            debounce_ms: default_debounce_ms(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DashboardConfig {
    #[serde(default = "default_dashboard_enabled")]
    pub enabled: bool,

    #[serde(default = "default_dashboard_addr")]
    pub listen_addr: String,

    #[serde(default = "default_dashboard_port")]
    pub port: u16,
}

impl Default for DashboardConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            listen_addr: default_dashboard_addr(),
            port: default_dashboard_port(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StageDef {
    pub name: String,

    #[serde(default)]
    pub depends_on: Vec<String>,

    #[serde(default)]
    pub commands: Vec<String>,

    #[serde(default)]
    pub artifacts: Vec<String>,

    #[serde(default = "default_cache_key")]
    pub cache_key: String,

    #[serde(default = "default_timeout_secs")]
    pub timeout_secs: u64,

    #[serde(default)]
    pub env: std::collections::HashMap<String, String>,

    #[serde(default)]
    pub allow_failure: bool,
}

fn default_name() -> String {
    "continuum".to_string()
}

fn default_work_dir() -> PathBuf {
    std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."))
}

fn default_dist_dir() -> PathBuf {
    PathBuf::from("target/ci-release")
}

fn default_data_dir() -> PathBuf {
    dirs_or_default()
}

fn dirs_or_default() -> PathBuf {
    dirs::data_dir()
        .map(|p| p.join("continuum-ci"))
        .unwrap_or_else(|| PathBuf::from(".continuum-ci"))
}

fn default_max_parallel() -> usize {
    4
}

fn default_timeout_secs() -> u64 {
    600
}

fn default_true() -> bool {
    true
}

fn default_branch_filter() -> String {
    ".*".to_string()
}

fn default_watch_paths() -> Vec<PathBuf> {
    vec![PathBuf::from("src"), PathBuf::from("Cargo.toml")]
}

fn default_ignore_patterns() -> Vec<String> {
    vec!["target/".to_string(), ".git/".to_string()]
}

fn default_debounce_ms() -> u64 {
    1000
}

fn default_dashboard_enabled() -> bool {
    true
}

fn default_dashboard_addr() -> String {
    "127.0.0.1".to_string()
}

fn default_dashboard_port() -> u16 {
    9090
}

fn default_cache_key() -> String {
    String::new()
}

impl CiConfig {
    pub fn load(path: Option<&std::path::Path>) -> anyhow::Result<Self> {
        let search_paths: Vec<PathBuf> = path.map(|p| vec![p.to_path_buf()]).unwrap_or_else(|| {
            vec![
                PathBuf::from("continuum-ci.toml"),
                PathBuf::from(".continuum-ci.toml"),
                PathBuf::from("ci.toml"),
            ]
        });

        for sp in &search_paths {
            if sp.exists() {
                let content = std::fs::read_to_string(sp)?;
                let mut config: CiConfig = toml::from_str(&content)?;
                let abs_work = std::fs::canonicalize(&config.project.work_dir)
                    .unwrap_or(config.project.work_dir.clone());
                config.project.work_dir = abs_work;
                tracing::info!(path = %sp.display(), "Loaded CI config");
                return Ok(config);
            }
        }

        tracing::warn!("No CI config found, using defaults");
        Ok(Self::default())
    }

    pub fn save_default(path: &std::path::Path) -> anyhow::Result<()> {
        let config = Self::default_pipeline();
        let toml_str = toml::to_string_pretty(&config)?;
        std::fs::write(path, toml_str)?;
        tracing::info!(path = %path.display(), "Saved default CI config");
        Ok(())
    }

    pub fn default_pipeline() -> Self {
        Self {
            project: ProjectConfig {
                name: "continuum".to_string(),
                work_dir: PathBuf::from("."),
                dist_dir: PathBuf::from("target/ci-release"),
                data_dir: dirs_or_default(),
                env: std::collections::HashMap::new(),
            },
            pipeline: PipelineConfig::default(),
            watch: WatchConfig::default(),
            dashboard: DashboardConfig::default(),
            stages: vec![
                StageDef {
                    name: "check".to_string(),
                    depends_on: vec![],
                    commands: vec!["cargo check --workspace".to_string()],
                    artifacts: vec![],
                    cache_key: String::new(),
                    timeout_secs: 300,
                    env: std::collections::HashMap::new(),
                    allow_failure: false,
                },
                StageDef {
                    name: "test".to_string(),
                    depends_on: vec!["check".to_string()],
                    commands: vec!["cargo test --workspace".to_string()],
                    artifacts: vec![],
                    cache_key: String::new(),
                    timeout_secs: 300,
                    env: std::collections::HashMap::new(),
                    allow_failure: false,
                },
                StageDef {
                    name: "clippy".to_string(),
                    depends_on: vec!["check".to_string()],
                    commands: vec![
                        "cargo clippy --workspace --all-targets -- -D warnings".to_string()
                    ],
                    artifacts: vec![],
                    cache_key: String::new(),
                    timeout_secs: 300,
                    env: std::collections::HashMap::new(),
                    allow_failure: true,
                },
                StageDef {
                    name: "fmt".to_string(),
                    depends_on: vec![],
                    commands: vec!["cargo fmt --all -- --check".to_string()],
                    artifacts: vec![],
                    cache_key: String::new(),
                    timeout_secs: 60,
                    env: std::collections::HashMap::new(),
                    allow_failure: true,
                },
                StageDef {
                    name: "build-release".to_string(),
                    depends_on: vec!["test".to_string()],
                    commands: vec![
                        "cargo build --release --bin continuum-server --bin continuum-client"
                            .to_string(),
                    ],
                    artifacts: vec!["target/release/continuum-server*".to_string()],
                    cache_key: String::new(),
                    timeout_secs: 600,
                    env: std::collections::HashMap::new(),
                    allow_failure: false,
                },
            ],
        }
    }
}
