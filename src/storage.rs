//! Azure Data Lake Storage Gen2 / Blob Storage client.
//!
//! Authentication relies on a **Storage Account Key** supplied via the
//! `AZURE_STORAGE_KEY` (or `AZURE_STORAGE_ACCESS_KEY`) environment variable.
//! The account name is passed explicitly.
//!
//! Internally the client wraps [`object_store`]'s `MicrosoftAzure` backend so
//! that the rest of the application is decoupled from any specific Azure SDK.

use std::sync::Arc;

use futures::TryStreamExt;
use globset::{Glob, GlobMatcher};
use object_store::{azure::MicrosoftAzureBuilder, path::Path as StorePath, ObjectStore};

use crate::error::{AppError, Result};

// ---------------------------------------------------------------------------
// StorageClient
// ---------------------------------------------------------------------------

/// Thin wrapper around an `ObjectStore` instance scoped to a single container.
pub struct StorageClient {
    store: Arc<dyn ObjectStore>,
}

impl StorageClient {
    /// Connect to Azure Storage using a Storage Account Key.
    ///
    /// # Environment variables
    ///
    /// | Variable                   | Description              |
    /// |----------------------------|--------------------------|
    /// | `AZURE_STORAGE_KEY`        | Account key (primary)    |
    /// | `AZURE_STORAGE_ACCESS_KEY` | Alternative key name     |
    ///
    /// # Errors
    ///
    /// Returns `AppError::Config` when the key variable is absent.  
    /// Returns `AppError::Storage` when the Azure builder rejects the config.
    pub fn new(account: &str, container: &str) -> Result<Self> {
        let access_key = std::env::var("AZURE_STORAGE_KEY")
            .or_else(|_| std::env::var("AZURE_STORAGE_ACCESS_KEY"))
            .map_err(|_| {
                AppError::Config(
                    "Azure Storage key not found. \
                     Please set the AZURE_STORAGE_KEY environment variable."
                        .to_string(),
                )
            })?;

        let store = MicrosoftAzureBuilder::new()
            .with_account(account)
            .with_access_key(access_key)
            .with_container_name(container)
            .build()?;

        Ok(Self {
            store: Arc::new(store),
        })
    }

    /// Construct from any pre-built `ObjectStore` implementation.
    ///
    /// Primarily used in tests to inject a `LocalFileSystem` backend.
    #[cfg(test)]
    pub fn from_store(store: Arc<dyn ObjectStore>) -> Self {
        Self { store }
    }

    /// List all blobs under `prefix` whose filename matches `file_pattern`.
    ///
    /// `file_pattern` is a glob applied to the **filename only** (e.g.
    /// `*.csv`, `orders_*.csv`).  Returns full object paths relative to the
    /// container root, sorted lexicographically for reproducible ordering.
    ///
    /// # Errors
    ///
    /// Returns `AppError::Rule` for an invalid glob pattern.  
    /// Returns `AppError::Storage` for any network or auth failure.
    pub async fn list_files(&self, prefix: &str, file_pattern: &str) -> Result<Vec<String>> {
        let glob_matcher = build_glob_matcher(file_pattern)?;
        let prefix_path = StorePath::from(prefix);

        let metas: Vec<_> = self
            .store
            .list(Some(&prefix_path))
            .try_collect()
            .await?;

        let mut paths: Vec<String> = metas
            .into_iter()
            .map(|m| m.location.to_string())
            .filter(|path| {
                // Match the glob against the filename component only.
                let filename = path.rsplit('/').next().unwrap_or(path);
                glob_matcher.is_match(filename)
            })
            .collect();

        paths.sort();
        Ok(paths)
    }

    /// Download the full contents of the object at `path`.
    ///
    /// # Errors
    ///
    /// Returns `AppError::Storage` if the object does not exist or cannot be
    /// read.
    pub async fn read_file(&self, path: &str) -> Result<Vec<u8>> {
        let location = StorePath::from(path);
        let result = self.store.get(&location).await?;
        let bytes = result.bytes().await?;
        Ok(bytes.to_vec())
    }
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn build_glob_matcher(pattern: &str) -> Result<GlobMatcher> {
    Glob::new(pattern)
        .map(|g| g.compile_matcher())
        .map_err(|e| AppError::Rule(format!("Invalid file pattern '{}': {}", pattern, e)))
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use object_store::memory::InMemory;
    use object_store::PutPayload;

    async fn in_memory_client(files: &[(&str, &[u8])]) -> StorageClient {
        let store = InMemory::new();
        for (path, data) in files {
            let location = StorePath::from(*path);
            let payload = PutPayload::from(data.to_vec());
            store.put(&location, payload).await.unwrap();
        }
        StorageClient::from_store(Arc::new(store))
    }

    #[tokio::test]
    async fn list_files_returns_matching_csvs() {
        let client = in_memory_client(&[
            ("raw/orders/2024-01-01.csv", b"a,b"),
            ("raw/orders/2024-01-02.csv", b"a,b"),
            ("raw/orders/README.txt", b"ignore me"),
        ])
        .await;

        let files = client.list_files("raw/orders", "*.csv").await.unwrap();
        assert_eq!(files.len(), 2);
        assert!(files.iter().all(|f| f.ends_with(".csv")));
    }

    #[tokio::test]
    async fn list_files_respects_glob_pattern() {
        let client = in_memory_client(&[
            ("raw/orders/orders_jan.csv", b"a"),
            ("raw/orders/summary.csv", b"b"),
        ])
        .await;

        let files = client
            .list_files("raw/orders", "orders_*.csv")
            .await
            .unwrap();
        assert_eq!(files.len(), 1);
        assert!(files[0].ends_with("orders_jan.csv"));
    }

    #[tokio::test]
    async fn read_file_returns_correct_bytes() {
        let content = b"col1,col2\nval1,val2\n";
        let client = in_memory_client(&[("raw/test.csv", content)]).await;

        let data = client.read_file("raw/test.csv").await.unwrap();
        assert_eq!(data, content);
    }

    #[tokio::test]
    async fn read_file_missing_path_returns_error() {
        let client = in_memory_client(&[]).await;
        let result = client.read_file("does/not/exist.csv").await;
        assert!(result.is_err());
    }
}
