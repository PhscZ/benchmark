//! Glicko-2 kill-rating service.
//!
//! * [`glicko`] — pure Glicko-2 maths.
//! * [`model`] — events, categories and login interning.
//! * [`engine`] — one chronological pass over the log producing all derived state.
//! * [`store`] — the event log, the alt map and every query the API needs.
//! * [`api`] — HTTP handlers; [`openapi`] — router and OpenAPI document.

pub mod api;
pub mod config;
pub mod engine;
pub mod error;
pub mod glicko;
pub mod model;
pub mod openapi;
pub mod store;

use crate::api::AppState;
use crate::config::Config;
use crate::store::{Store, StoreError};
use axum::Router;
use std::sync::{Arc, RwLock};

/// Builds the store for `config`, applying the optional seed files.
pub fn build_store(config: &Config) -> Result<Store, StoreError> {
    config.validate().map_err(StoreError::Invalid)?;
    let mut store = Store::load(config)?;
    if let Some(path) = &config.alts_file {
        let entries = config::read_json_array(path).map_err(StoreError::Internal)?;
        store.upsert_alts(entries)?;
    }
    if let Some(path) = &config.import_file {
        store.import_json_file(path)?;
    }
    Ok(store)
}

/// Builds shared application state around a store.
pub fn build_state(store: Store, config: Config) -> AppState {
    AppState {
        store: Arc::new(RwLock::new(store)),
        config: Arc::new(config),
    }
}

/// Builds the store, the state and the router in one call.
pub fn build_app(config: &Config) -> Result<Router, StoreError> {
    let store = build_store(config)?;
    Ok(openapi::router(build_state(store, config.clone())))
}
