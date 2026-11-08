#![forbid(unsafe_code)]

pub mod auth;
pub mod catalog;
pub mod config;
pub mod error;
mod media;
pub mod processing;
pub mod routes;
pub mod service;
mod uploads;

#[cfg(any(test, feature = "cpu-transcription"))]
mod transcription_authorization;

#[cfg(feature = "cpu-transcription")]
mod transcription_cpu;

#[cfg(test)]
mod tests;
