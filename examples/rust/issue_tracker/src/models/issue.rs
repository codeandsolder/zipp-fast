use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum IssueStatus {
    Open,
    InProgress,
    Closed,
}

impl IssueStatus {
    pub fn as_str(&self) -> &'static str {
        match self {
            IssueStatus::Open => "open",
            IssueStatus::InProgress => "in_progress",
            IssueStatus::Closed => "closed",
        }
    }

    pub fn parse(s: &str) -> Result<Self, String> {
        match s.trim().to_lowercase().as_str() {
            "open" => Ok(IssueStatus::Open),
            "in_progress" | "in-progress" | "inprogress" => Ok(IssueStatus::InProgress),
            "closed" => Ok(IssueStatus::Closed),
            other => Err(format!("invalid issue status `{other}`")),
        }
    }

    /// Validates and applies a status transition in-place via `&mut self`.
    pub fn transition_to(&mut self, target: IssueStatus) -> Result<(), String> {
        let allowed = match self {
            IssueStatus::Open => matches!(target, IssueStatus::Open | IssueStatus::InProgress | IssueStatus::Closed),
            IssueStatus::InProgress => matches!(target, IssueStatus::InProgress | IssueStatus::Open | IssueStatus::Closed),
            IssueStatus::Closed => matches!(target, IssueStatus::Closed | IssueStatus::Open),
        };
        if !allowed {
            return Err(format!(
                "cannot transition directly from `{}` to `{}` (reopen first)",
                self.as_str(),
                target.as_str()
            ));
        }
        *self = target;
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Priority {
    Low,
    Medium,
    High,
    Critical,
}

impl Priority {
    pub fn as_str(&self) -> &'static str {
        match self {
            Priority::Low => "low",
            Priority::Medium => "medium",
            Priority::High => "high",
            Priority::Critical => "critical",
        }
    }

    pub fn parse(s: &str) -> Result<Self, String> {
        match s.trim().to_lowercase().as_str() {
            "low" => Ok(Priority::Low),
            "medium" => Ok(Priority::Medium),
            "high" => Ok(Priority::High),
            "critical" => Ok(Priority::Critical),
            other => Err(format!("invalid priority `{other}`")),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Comment {
    pub id: i64,
    pub author: String,
    pub body: String,
    pub created_at: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Issue {
    pub id: i64,
    pub title: String,
    pub description: String,
    pub status: IssueStatus,
    pub priority: Priority,
    pub author: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub assignee: Option<String>,
    pub labels: Vec<String>,
    pub comments: Vec<Comment>,
    pub created_at: String,
    pub updated_at: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CreateIssueRequest {
    pub title: String,
    pub description: Option<String>,
    pub priority: Option<Priority>,
    pub assignee: Option<String>,
    pub labels: Option<Vec<String>>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct UpdateIssueRequest {
    pub title: Option<String>,
    pub description: Option<String>,
    pub status: Option<IssueStatus>,
    pub priority: Option<Priority>,
    pub assignee: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AddCommentRequest {
    pub body: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct IssueStats {
    pub total: i64,
    pub open: i64,
    pub in_progress: i64,
    pub closed: i64,
    pub users: i64,
}
