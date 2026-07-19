use std::{ops::Range, sync::Arc, time::Duration};

use bytes::Bytes;
use object_store::{
    GetOptions, GetRange, GetResult, ObjectStore,
    aws::{AmazonS3, AmazonS3Builder},
    path::Path,
};
use secrecy::ExposeSecret;

use crate::{configuration::ObjectStoreSettings, features::core::errors::AppError};

#[derive(Clone)]
pub struct RustFsClient {
    files_bucket: String,
    audit_bucket: String,
    files_store: Option<Arc<AmazonS3>>,
    audit_store: Option<Arc<AmazonS3>>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ObjectWriteResult {
    pub bucket: String,
    pub object_key: String,
    pub version_id: Option<String>,
    pub etag: Option<String>,
}

impl RustFsClient {
    pub fn new(settings: ObjectStoreSettings) -> Result<Self, object_store::Error> {
        let files_bucket = settings.files_bucket.clone();
        let audit_bucket = settings.audit_bucket.clone();
        let files_store = if settings.enabled {
            Some(Arc::new(build_s3_store(&settings, &files_bucket)?))
        } else {
            None
        };
        let audit_store = if settings.enabled {
            Some(Arc::new(build_s3_store(&settings, &audit_bucket)?))
        } else {
            None
        };

        Ok(Self {
            files_bucket,
            audit_bucket,
            files_store,
            audit_store,
        })
    }

    pub fn disabled_for_tests() -> Self {
        Self {
            files_bucket: "bibi-work-files".to_string(),
            audit_bucket: "bibi-work-audit".to_string(),
            files_store: None,
            audit_store: None,
        }
    }

    pub fn is_enabled(&self) -> bool {
        self.files_store.is_some()
    }

    pub fn files_bucket(&self) -> &str {
        &self.files_bucket
    }

    pub fn audit_bucket(&self) -> &str {
        &self.audit_bucket
    }

    pub async fn put_file_object(
        &self,
        object_key: &str,
        content: Vec<u8>,
    ) -> Result<Option<ObjectWriteResult>, AppError> {
        self.put_object(object_key, content).await
    }

    pub async fn put_object(
        &self,
        object_key: &str,
        content: Vec<u8>,
    ) -> Result<Option<ObjectWriteResult>, AppError> {
        put_to_store(&self.files_store, &self.files_bucket, object_key, content).await
    }

    pub async fn put_audit_object(
        &self,
        object_key: &str,
        content: Vec<u8>,
    ) -> Result<Option<ObjectWriteResult>, AppError> {
        put_to_store(&self.audit_store, &self.audit_bucket, object_key, content).await
    }

    pub async fn get_file_object(&self, object_key: &str) -> Result<Option<Vec<u8>>, AppError> {
        self.get_object_version(object_key, None).await
    }

    pub async fn get_file_object_version(
        &self,
        object_key: &str,
        version_id: Option<&str>,
    ) -> Result<Option<Vec<u8>>, AppError> {
        self.get_object_version(object_key, version_id).await
    }

    pub async fn get_file_object_range_version(
        &self,
        object_key: &str,
        version_id: Option<&str>,
        offset_bytes: u64,
        limit_bytes: usize,
    ) -> Result<Option<Vec<u8>>, AppError> {
        get_from_store_range(
            &self.files_store,
            object_key,
            version_id,
            offset_bytes,
            limit_bytes,
        )
        .await
    }

    pub async fn get_file_object_stream_version(
        &self,
        object_key: &str,
        version_id: Option<&str>,
        range: Option<Range<u64>>,
    ) -> Result<Option<GetResult>, AppError> {
        get_from_store_stream(&self.files_store, object_key, version_id, range).await
    }

    pub async fn get_object(&self, object_key: &str) -> Result<Option<Vec<u8>>, AppError> {
        self.get_object_version(object_key, None).await
    }

    pub async fn get_object_version(
        &self,
        object_key: &str,
        version_id: Option<&str>,
    ) -> Result<Option<Vec<u8>>, AppError> {
        get_from_store(&self.files_store, object_key, version_id).await
    }

    pub async fn get_audit_object(&self, object_key: &str) -> Result<Option<Vec<u8>>, AppError> {
        get_from_store(&self.audit_store, object_key, None).await
    }

    pub async fn delete_file_object(&self, object_key: &str) -> Result<(), AppError> {
        self.delete_object(object_key).await
    }

    pub async fn delete_object(&self, object_key: &str) -> Result<(), AppError> {
        delete_from_store(&self.files_store, object_key).await
    }

    pub async fn delete_audit_object(&self, object_key: &str) -> Result<(), AppError> {
        delete_from_store(&self.audit_store, object_key).await
    }
}

fn build_s3_store(
    settings: &ObjectStoreSettings,
    bucket: &str,
) -> Result<AmazonS3, object_store::Error> {
    AmazonS3Builder::new()
        .with_endpoint(settings.endpoint.clone())
        .with_access_key_id(settings.access_key.expose_secret())
        .with_secret_access_key(settings.secret_key.expose_secret())
        .with_region(settings.region.clone())
        .with_bucket_name(bucket)
        .with_client_options(
            object_store::ClientOptions::new()
                .with_allow_http(true)
                .with_timeout(Duration::from_millis(settings.timeout_milliseconds)),
        )
        .build()
}

async fn put_to_store(
    store: &Option<Arc<AmazonS3>>,
    bucket: &str,
    object_key: &str,
    content: Vec<u8>,
) -> Result<Option<ObjectWriteResult>, AppError> {
    let Some(store) = store else {
        return Ok(None);
    };

    let result = store
        .put(&Path::from(object_key), Bytes::from(content).into())
        .await
        .map_err(object_store_error)?;

    Ok(Some(ObjectWriteResult {
        bucket: bucket.to_string(),
        object_key: object_key.to_string(),
        version_id: result.version,
        etag: result.e_tag,
    }))
}

async fn get_from_store(
    store: &Option<Arc<AmazonS3>>,
    object_key: &str,
    version_id: Option<&str>,
) -> Result<Option<Vec<u8>>, AppError> {
    let Some(store) = store else {
        return Ok(None);
    };

    let options = GetOptions {
        version: version_id.map(str::to_string),
        ..Default::default()
    };
    let bytes = store
        .get_opts(&Path::from(object_key), options)
        .await
        .map_err(object_store_error)?
        .bytes()
        .await
        .map_err(object_store_error)?;

    Ok(Some(bytes.to_vec()))
}

async fn get_from_store_range(
    store: &Option<Arc<AmazonS3>>,
    object_key: &str,
    version_id: Option<&str>,
    offset_bytes: u64,
    limit_bytes: usize,
) -> Result<Option<Vec<u8>>, AppError> {
    let Some(store) = store else {
        return Ok(None);
    };
    if limit_bytes == 0 {
        return Ok(Some(Vec::new()));
    }
    let end = offset_bytes
        .checked_add(u64::try_from(limit_bytes)?)
        .ok_or_else(|| AppError::InvalidInput("range end is invalid".to_string()))?;
    let options = GetOptions {
        version: version_id.map(str::to_string),
        range: Some(GetRange::Bounded(offset_bytes..end)),
        ..Default::default()
    };
    let bytes = store
        .get_opts(&Path::from(object_key), options)
        .await
        .map_err(object_store_error)?
        .bytes()
        .await
        .map_err(object_store_error)?;

    Ok(Some(bytes.to_vec()))
}

async fn get_from_store_stream(
    store: &Option<Arc<AmazonS3>>,
    object_key: &str,
    version_id: Option<&str>,
    range: Option<Range<u64>>,
) -> Result<Option<GetResult>, AppError> {
    let Some(store) = store else {
        return Ok(None);
    };
    let options = GetOptions {
        version: version_id.map(str::to_string),
        range: range.map(GetRange::Bounded),
        ..Default::default()
    };
    let result = store
        .get_opts(&Path::from(object_key), options)
        .await
        .map_err(object_store_error)?;

    Ok(Some(result))
}

async fn delete_from_store(
    store: &Option<Arc<AmazonS3>>,
    object_key: &str,
) -> Result<(), AppError> {
    let Some(store) = store else {
        return Ok(());
    };

    store
        .delete(&Path::from(object_key))
        .await
        .map_err(object_store_error)
}

fn object_store_error(err: object_store::Error) -> AppError {
    AppError::ObjectStore(format!("{err:?}"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::configuration::ObjectStoreSettings;
    use secrecy::SecretBox;
    use uuid::Uuid;

    #[test]
    fn disabled_client_skips_object_store_calls() {
        let client = RustFsClient::disabled_for_tests();
        assert!(!client.is_enabled());
        assert_eq!(client.files_bucket(), "bibi-work-files");
        assert_eq!(client.audit_bucket(), "bibi-work-audit");
    }

    #[tokio::test]
    async fn disabled_client_skips_range_reads() {
        let client = RustFsClient::disabled_for_tests();
        let content = client
            .get_file_object_range_version("missing", None, 0, 10)
            .await
            .expect("range read");

        assert_eq!(content, None);
    }

    #[tokio::test]
    #[ignore = "requires local RustFS"]
    async fn local_rustfs_puts_and_reads_file_object() {
        let client = RustFsClient::new(ObjectStoreSettings {
            enabled: true,
            endpoint: std::env::var("RUSTFS_ENDPOINT")
                .unwrap_or_else(|_| "http://localhost:9004".to_string()),
            access_key: secret(
                &std::env::var("RUSTFS_ACCESS_KEY").unwrap_or_else(|_| "rustfsadmin".to_string()),
            ),
            secret_key: secret(
                &std::env::var("RUSTFS_SECRET_KEY").unwrap_or_else(|_| "rustfsadmin".to_string()),
            ),
            region: std::env::var("RUSTFS_REGION").unwrap_or_else(|_| "us-east-1".to_string()),
            files_bucket: std::env::var("RUSTFS_FILES_BUCKET")
                .unwrap_or_else(|_| "bibi-work-files".to_string()),
            audit_bucket: std::env::var("RUSTFS_AUDIT_BUCKET")
                .unwrap_or_else(|_| "bibi-work-audit".to_string()),
            timeout_milliseconds: 5000,
        })
        .expect("RustFS client");

        let object_key = format!(
            "tenants/bibi-work/test/rustfs-client/{}.txt",
            Uuid::new_v4()
        );
        let content = b"rustfs client e2e".to_vec();

        let result = client
            .put_file_object(&object_key, content.clone())
            .await
            .expect("put object")
            .expect("enabled object store");
        assert_eq!(result.object_key, object_key);

        let stored = client
            .get_file_object(&object_key)
            .await
            .expect("get object");
        assert_eq!(stored, Some(content));

        let stored_by_version = client
            .get_file_object_version(&object_key, result.version_id.as_deref())
            .await
            .expect("get object by version");
        assert_eq!(stored_by_version, Some(b"rustfs client e2e".to_vec()));

        client
            .delete_file_object(&object_key)
            .await
            .expect("delete object");
    }

    fn secret(value: &str) -> SecretBox<str> {
        SecretBox::new(value.to_string().into_boxed_str())
    }
}
