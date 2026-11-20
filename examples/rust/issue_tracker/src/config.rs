use std::env;

#[derive(Debug, Clone)]
pub struct AppConfig {
    pub app_name: String,
    pub app_version: String,
    pub jwt_secret: String,
    pub data_file: String,
    pub bind_addr: String,
}

impl AppConfig {
    pub fn from_env() -> Self {
        let app_name = env!("CARGO_PKG_NAME").to_string();
        let app_version = env!("CARGO_PKG_VERSION").to_string();
        let jwt_secret = env::var("JWT_SECRET")
            .unwrap_or_else(|_| String::from("zipp-issue-tracker-dev-secret"));
        let data_file = env::var("ISSUE_DATA_FILE")
            .unwrap_or_else(|_| String::from("data/issues.json"));
        let bind_addr = env::var("BIND_ADDR")
            .unwrap_or_else(|_| String::from("127.0.0.1:8080"));
        Self {
            app_name,
            app_version,
            jwt_secret,
            data_file,
            bind_addr,
        }
    }
}
