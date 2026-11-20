use std::fs;
use std::path::Path;
use serde::{Deserialize, Serialize};

use crate::auth::{hash_password, verify_password};
use crate::error::AppError;
use crate::models::{
    AddCommentRequest, Comment, CreateIssueRequest, Issue, IssueStats, IssueStatus, Priority,
    UpdateIssueRequest, User, UserRecord,
};

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct StoreSnapshot {
    next_user_id: i64,
    next_issue_id: i64,
    next_comment_id: i64,
    users: Vec<UserRecord>,
    issues: Vec<Issue>,
}

pub struct IssueStore {
    path: Option<String>,
    data: StoreSnapshot,
}

impl IssueStore {
    pub fn in_memory() -> Self {
        Self {
            path: None,
            data: StoreSnapshot {
                next_user_id: 1,
                next_issue_id: 1,
                next_comment_id: 1,
                users: Vec::new(),
                issues: Vec::new(),
            },
        }
    }

    pub fn open(path: &str) -> Result<Self, AppError> {
        let p = Path::new(path);
        if p.exists() {
            let raw = fs::read_to_string(path)?;
            if !raw.trim().is_empty() {
                let data = serde_json::from_str::<StoreSnapshot>(&raw)
                    .map_err(|e| AppError::Internal(format!("corrupt store `{path}`: {e}")))?;
                return Ok(Self {
                    path: Some(path.to_string()),
                    data,
                });
            }
        }
        Ok(Self {
            path: Some(path.to_string()),
            data: StoreSnapshot {
                next_user_id: 1,
                next_issue_id: 1,
                next_comment_id: 1,
                users: Vec::new(),
                issues: Vec::new(),
            },
        })
    }

    fn persist(&self) -> Result<(), AppError> {
        if let Some(ref path) = self.path {
            if let Some(parent) = Path::new(path).parent() {
                let parent_str = parent.to_string_lossy();
                if !parent_str.is_empty() && parent_str != "." {
                    let _ = fs::create_dir_all(&parent_str);
                }
            }
            let json = serde_json::to_string_pretty(&self.data)
                .map_err(|e| AppError::Internal(format!("serialize error: {e}")))?;
            fs::write(path, json)?;
        }
        Ok(())
    }

    pub fn register_user(&mut self, username: &str, password: &str) -> Result<UserRecord, AppError> {
        let uname = username.trim().to_string();
        if uname.len() < 2 {
            return Err(AppError::BadRequest(String::from(
                "username must be at least 2 characters",
            )));
        }
        if password.len() < 6 {
            return Err(AppError::BadRequest(String::from(
                "password must be at least 6 characters",
            )));
        }
        for u in &self.data.users {
            if u.username.to_lowercase() == uname.to_lowercase() {
                return Err(AppError::Conflict(format!("username `{uname}` is already taken")));
            }
        }
        let id = self.data.next_user_id;
        self.data.next_user_id += 1;
        let role = if id == 1 { String::from("admin") } else { String::from("member") };
        let rec = UserRecord {
            id,
            username: uname,
            password_hash: hash_password(password),
            role,
            created_at: Utc::now().to_rfc3339(),
        };
        self.data.users.push(rec.clone());
        self.persist()?;
        Ok(rec)
    }

    pub fn authenticate(&self, username: &str, password: &str) -> Result<UserRecord, AppError> {
        let needle = username.trim().to_lowercase();
        for u in &self.data.users {
            if u.username.to_lowercase() == needle {
                if verify_password(password, &u.password_hash) {
                    return Ok(u.clone());
                }
                return Err(AppError::Unauthorized(String::from("invalid username or password")));
            }
        }
        Err(AppError::Unauthorized(String::from("invalid username or password")))
    }

    pub fn find_user_by_id(&self, id: i64) -> Option<User> {
        for u in &self.data.users {
            if u.id == id {
                return Some(User::from(u.clone()));
            }
        }
        None
    }

    pub fn create_issue(&mut self, author: &str, req: CreateIssueRequest) -> Result<Issue, AppError> {
        let title = req.title.trim().to_string();
        if title.is_empty() {
            return Err(AppError::BadRequest(String::from("issue title cannot be empty")));
        }
        let id = self.data.next_issue_id;
        self.data.next_issue_id += 1;
        let now = Utc::now().to_rfc3339();
        let issue = Issue {
            id,
            title,
            description: req.description.unwrap_or_default(),
            status: IssueStatus::Open,
            priority: req.priority.unwrap_or(Priority::Medium),
            author: author.to_string(),
            assignee: req.assignee,
            labels: req.labels.unwrap_or_default(),
            comments: Vec::new(),
            created_at: now.clone(),
            updated_at: now,
        };
        self.data.issues.push(issue.clone());
        self.persist()?;
        Ok(issue)
    }

    pub fn list_issues(
        &self,
        status_filter: Option<IssueStatus>,
        priority_filter: Option<Priority>,
        query: Option<String>,
    ) -> Vec<Issue> {
        let q_lower = query.map(|s| s.trim().to_lowercase()).filter(|s| !s.is_empty());
        let mut out = Vec::new();
        for issue in &self.data.issues {
            if let Some(ref st) = status_filter {
                if issue.status != *st {
                    continue;
                }
            }
            if let Some(ref pr) = priority_filter {
                if issue.priority != *pr {
                    continue;
                }
            }
            if let Some(ref needle) = q_lower {
                let in_title = issue.title.to_lowercase().contains(needle.as_str());
                let in_desc = issue.description.to_lowercase().contains(needle.as_str());
                let in_author = issue.author.to_lowercase().contains(needle.as_str());
                if !in_title && !in_desc && !in_author {
                    continue;
                }
            }
            out.push(issue.clone());
        }
        out
    }

    pub fn get_issue(&self, id: i64) -> Result<Issue, AppError> {
        for issue in &self.data.issues {
            if issue.id == id {
                return Ok(issue.clone());
            }
        }
        Err(AppError::NotFound(format!("issue #{id} not found")))
    }

    pub fn update_issue(&mut self, id: i64, req: UpdateIssueRequest) -> Result<Issue, AppError> {
        let mut idx_opt: Option<usize> = None;
        for (i, issue) in self.data.issues.iter().enumerate() {
            if issue.id == id {
                idx_opt = Some(i);
                break;
            }
        }
        let Some(idx) = idx_opt else {
            return Err(AppError::NotFound(format!("issue #{id} not found")));
        };
        if let Some(title) = req.title {
            let trimmed = title.trim().to_string();
            if trimmed.is_empty() {
                return Err(AppError::BadRequest(String::from("title cannot be empty")));
            }
            self.data.issues[idx].title = trimmed;
        }
        if let Some(desc) = req.description {
            self.data.issues[idx].description = desc;
        }
        if let Some(new_status) = req.status {
            self.data.issues[idx].status.transition_to(new_status)?;
        }
        if let Some(priority) = req.priority {
            self.data.issues[idx].priority = priority;
        }
        if let Some(assignee) = req.assignee {
            self.data.issues[idx].assignee = if assignee.trim().is_empty() {
                None
            } else {
                Some(assignee.trim().to_string())
            };
        }
        self.data.issues[idx].updated_at = Utc::now().to_rfc3339();
        let updated = self.data.issues[idx].clone();
        self.persist()?;
        Ok(updated)
    }

    pub fn add_comment(
        &mut self,
        issue_id: i64,
        author: &str,
        req: AddCommentRequest,
    ) -> Result<Comment, AppError> {
        let body = req.body.trim().to_string();
        if body.is_empty() {
            return Err(AppError::BadRequest(String::from("comment body cannot be empty")));
        }
        let mut idx_opt: Option<usize> = None;
        for (i, issue) in self.data.issues.iter().enumerate() {
            if issue.id == issue_id {
                idx_opt = Some(i);
                break;
            }
        }
        let Some(idx) = idx_opt else {
            return Err(AppError::NotFound(format!("issue #{issue_id} not found")));
        };
        let cid = self.data.next_comment_id;
        self.data.next_comment_id += 1;
        let comment = Comment {
            id: cid,
            author: author.to_string(),
            body,
            created_at: Utc::now().to_rfc3339(),
        };
        self.data.issues[idx].comments.push(comment.clone());
        self.data.issues[idx].updated_at = Utc::now().to_rfc3339();
        self.persist()?;
        Ok(comment)
    }

    pub fn delete_issue(&mut self, id: i64) -> Result<(), AppError> {
        let before = self.data.issues.len();
        self.data.issues.retain(|issue| issue.id != id);
        if self.data.issues.len() == before {
            return Err(AppError::NotFound(format!("issue #{id} not found")));
        }
        self.persist()?;
        Ok(())
    }

    pub fn stats(&self) -> IssueStats {
        let mut open = 0;
        let mut in_progress = 0;
        let mut closed = 0;
        for issue in &self.data.issues {
            match issue.status {
                IssueStatus::Open => open += 1,
                IssueStatus::InProgress => in_progress += 1,
                IssueStatus::Closed => closed += 1,
            }
        }
        IssueStats {
            total: self.data.issues.len() as i64,
            open,
            in_progress,
            closed,
            users: self.data.users.len() as i64,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::auth::{issue_token, verify_token};
    use crate::config::AppConfig;

    #[test]
    fn test_user_registration_and_password_verification() {
        let mut store = IssueStore::in_memory();
        let rec = store.register_user("alice", "hunter22").unwrap();
        assert_eq!(rec.id, 1);
        assert_eq!(rec.role, "admin");
        assert!(rec.password_hash.starts_with("$zsha256$"));

        // Duplicate username rejected
        assert!(store.register_user("ALICE", "anotherpw").is_err());

        // Auth succeeds with right password and fails with wrong password
        let ok = store.authenticate("alice", "hunter22").unwrap();
        assert_eq!(ok.username, "alice");
        assert!(store.authenticate("alice", "wrongpass").is_err());

        // Public User view hides password_hash during JSON serialization
        let pub_user: User = rec.into();
        let serialized = serde_json::to_string(&pub_user).unwrap();
        assert!(!serialized.contains("passwordHash"));
        assert!(!serialized.contains("$zsha256$"));
        assert!(serialized.contains("\"username\":\"alice\""));
    }

    #[test]
    fn test_issue_lifecycle_status_transitions_and_filtering() {
        let mut store = IssueStore::in_memory();
        let i1 = store
            .create_issue(
                "alice",
                CreateIssueRequest {
                    title: String::from("Add Rust JIT tests"),
                    description: Some(String::from("Verify hot loop speed")),
                    priority: Some(Priority::High),
                    assignee: Some(String::from("bob")),
                    labels: Some(vec![String::from("vm"), String::from("rust")]),
                },
            )
            .unwrap();
        assert_eq!(i1.id, 1);
        assert_eq!(i1.status, IssueStatus::Open);

        // Open -> InProgress -> Closed
        let u1 = store
            .update_issue(
                1,
                UpdateIssueRequest {
                    title: None,
                    description: None,
                    status: Some(IssueStatus::InProgress),
                    priority: None,
                    assignee: None,
                },
            )
            .unwrap();
        assert_eq!(u1.status, IssueStatus::InProgress);

        let u2 = store
            .update_issue(
                1,
                UpdateIssueRequest {
                    title: None,
                    description: None,
                    status: Some(IssueStatus::Closed),
                    priority: None,
                    assignee: None,
                },
            )
            .unwrap();
        assert_eq!(u2.status, IssueStatus::Closed);

        // Invalid direct transition Closed -> InProgress rejected!
        let bad = store.update_issue(
            1,
            UpdateIssueRequest {
                title: None,
                description: None,
                status: Some(IssueStatus::InProgress),
                priority: None,
                assignee: None,
            },
        );
        assert!(bad.is_err());

        // Comment + query filter + stats
        let c = store
            .add_comment(
                1,
                "bob",
                AddCommentRequest {
                    body: String::from("Looks great!"),
                },
            )
            .unwrap();
        assert_eq!(c.id, 1);

        let found = store.list_issues(Some(IssueStatus::Closed), None, Some(String::from("jit")));
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].comments.len(), 1);

        let stats = store.stats();
        assert_eq!(stats.total, 1);
        assert_eq!(stats.closed, 1);
    }

    #[test]
    async fn test_async_jwt_roundtrip() {
        let mut store = IssueStore::in_memory();
        let rec = store.register_user("grace", "compiler123").unwrap();
        let cfg = AppConfig::from_env();
        let token = async { issue_token(&rec, &cfg) }.await.unwrap();
        let claims = verify_token(&token, &cfg).unwrap();
        assert_eq!(claims.user_id, rec.id);
        assert_eq!(claims.username, "grace");
    }
}
