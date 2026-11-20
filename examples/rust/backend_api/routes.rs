//! Route handlers for the multi-file ZIPP Rust backend API example.

use crate::models::{AppState, Item};
use http::{Request, Response};
use serde_json::json;
use std::sync::{Arc, Mutex};

pub fn health(_req: Request, state: Arc<Mutex<AppState>>) -> Response {
    let mut st = state.lock().unwrap();
    st.request_count += 1;
    Response::json(json!({
        "status": "ok",
        "runtime": "zipp-rust",
        "requests": st.request_count,
        "item_count": st.items.len()
    }))
}

pub fn list_items(_req: Request, state: Arc<Mutex<AppState>>) -> Response {
    let st = state.lock().unwrap();
    Response::json(&st.items)
}

pub fn get_item(req: Request, state: Arc<Mutex<AppState>>) -> Response {
    let id = req.param("id").unwrap_or_default();
    let st = state.lock().unwrap();
    for item in st.items.iter() {
        if item.id == id {
            return Response::json(item);
        }
    }
    Response::not_found(format!("item '{id}' not found"))
}

pub fn create_item(req: Request, state: Arc<Mutex<AppState>>) -> Response {
    let Ok(payload) = req.json::<serde_json::Value>() else {
        return Response::bad_request("invalid JSON payload");
    };
    let Some(name) = payload["name"].as_str() else {
        return Response::bad_request("missing field `name`");
    };
    let qty = payload["quantity"].as_i64().unwrap_or(1);
    let id = format!("item-{}", &crypto::uuid_v4()[..8]);
    let item = Item::new(id, name.to_string(), qty);
    let mut st = state.lock().unwrap();
    st.items.push(item.clone());
    Response::json(&item).with_status(201)
}
