#![forbid(unsafe_code)]

pub mod auth;
pub mod catalog;
pub mod config;
pub mod error;
pub mod routes;
pub mod service;

#[cfg(test)]
mod tests;
