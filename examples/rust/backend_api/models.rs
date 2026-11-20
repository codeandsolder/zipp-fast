//! Domain models for the multi-file ZIPP Rust backend API example.

#[derive(Debug, Clone)]
pub struct Item {
    pub id: String,
    pub name: String,
    pub quantity: i64,
}

impl Item {
    pub fn new(id: String, name: String, quantity: i64) -> Self {
        Self { id, name, quantity }
    }
}

pub struct AppState {
    pub items: Vec<Item>,
    pub request_count: i64,
}

impl AppState {
    pub fn seeded() -> Self {
        Self {
            items: vec![
                Item::new("item-1".to_string(), "Widget".to_string(), 10),
                Item::new("item-2".to_string(), "Gadget".to_string(), 5),
            ],
            request_count: 0,
        }
    }
}
