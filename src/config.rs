use serde_json::Value;

#[derive(Debug, Clone, Copy)]
pub enum WorkspaceMode {
    OpenFilesOnly,
}

#[derive(Debug, Clone, Copy)]
pub enum LogLevel {
    Error,
    Warn,
    Info,
    Debug,
}

#[derive(Debug, Clone)]
pub struct Config {
    pub workspace_mode: WorkspaceMode,
    pub check_on_save: bool,
    pub check_command: Vec<String>,
    pub log_level: LogLevel,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            workspace_mode: WorkspaceMode::OpenFilesOnly,
            check_on_save: true,
            check_command: vec![
                "cargo".to_string(),
                "check".to_string(),
                "-q".to_string(),
                "--message-format=json".to_string(),
            ],
            log_level: LogLevel::Warn,
        }
    }
}

impl Config {
    pub fn update_from_settings(&mut self, settings: &Value) {
        let root = if let Some(obj) = settings.as_object() {
            if let Some(inner) = obj.get("stapler") {
                inner
            } else {
                settings
            }
        } else {
            settings
        };

        if let Some(mode) = root.get("workspaceMode").and_then(|v| v.as_str()) {
            if mode.eq_ignore_ascii_case("openFilesOnly") {
                self.workspace_mode = WorkspaceMode::OpenFilesOnly;
            }
        }

        if let Some(check) = root.get("checkOnSave").and_then(|v| v.as_bool()) {
            self.check_on_save = check;
        }

        if let Some(cmd) = root.get("checkCommand") {
            if let Some(arr) = cmd.as_array() {
                let mut next = Vec::new();
                for item in arr {
                    if let Some(s) = item.as_str() {
                        next.push(s.to_string());
                    }
                }
                if !next.is_empty() {
                    self.check_command = next;
                }
            }
        }

        if let Some(level) = root.get("logLevel").and_then(|v| v.as_str()) {
            self.log_level = match level.to_ascii_lowercase().as_str() {
                "error" => LogLevel::Error,
                "info" => LogLevel::Info,
                "debug" => LogLevel::Debug,
                _ => LogLevel::Warn,
            };
        }
    }
}
