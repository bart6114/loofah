#![forbid(unsafe_code)]

pub mod auth;
pub mod catalog;
pub mod config;
pub mod error;
pub mod maintenance;
mod media;
pub mod processing;
pub mod routes;
pub mod service;
mod uploads;

mod media_jobs;
mod transcription_authorization;

#[cfg(feature = "cpu-transcription")]
mod transcription_cpu;

#[cfg(feature = "cpu-transcription")]
mod transcription_host;

#[cfg(test)]
mod tests;
