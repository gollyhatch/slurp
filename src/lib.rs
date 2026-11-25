//! A terminal EPUB reader.

pub mod app;
pub mod book;
pub mod config;
pub mod html;
pub mod layout;
pub mod library;
pub mod state;
pub mod sync;
pub mod theme;

#[cfg(test)]
pub(crate) mod testutil;
