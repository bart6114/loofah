#![forbid(unsafe_code)]

pub mod auth;
pub mod catalog;
pub mod config;
pub mod error;
mod media;
pub mod routes;
pub mod service;
mod uploads;

#[cfg(test)]
mod tests;
