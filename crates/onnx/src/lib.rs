mod error;
pub use error::*;

use ort::{
    Result,
    session::{
        Session,
        builder::{GraphOptimizationLevel, SessionBuilder},
    },
};

pub use ndarray;
pub use ort;

fn session_builder() -> Result<SessionBuilder> {
    Ok(Session::builder()?
        .with_intra_threads(1)?
        .with_inter_threads(1)?
        .with_optimization_level(GraphOptimizationLevel::Level3)?)
}

pub fn load_model_from_bytes(bytes: &[u8]) -> Result<Session, Error> {
    Ok(session_builder()?.commit_from_memory(bytes)?)
}

pub fn load_model_from_path(path: impl AsRef<std::path::Path>) -> Result<Session, Error> {
    Ok(session_builder()?.commit_from_file(path)?)
}
