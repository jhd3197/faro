use crate::profiles::{AuthMethod, ConnectionProfile};
use anyhow::{anyhow, Context, Result};
use object_store::aws::AmazonS3Builder;
use object_store::ObjectStore;
use std::sync::Arc;
use uuid::Uuid;

/// An S3-compatible object store session. The same backing crate handles AWS
/// S3, Cloudflare R2, and Backblaze B2 — only the endpoint URL and region
/// hint differ. v0.6 will add Azure Blob via object_store's azure feature
/// (different builder, same trait).
pub struct S3Session {
    pub id: String,
    pub profile: ConnectionProfile,
    pub bucket: String,
    /// The boxed trait object lets transfer.rs and remotefs/s3.rs share one
    /// client without locking on a concrete type.
    pub store: Arc<dyn ObjectStore>,
}

pub async fn s3_connect(profile: &ConnectionProfile) -> Result<S3Session> {
    let bucket = profile
        .bucket
        .as_ref()
        .ok_or_else(|| anyhow!("S3 profile is missing the bucket name"))?
        .clone();
    let access_key = profile.username.clone();
    let secret_key = match &profile.auth {
        AuthMethod::Password { password } => password.clone(),
        _ => {
            return Err(anyhow!(
                "S3 backend requires an access key (username) and secret (password). \
                 Switch the auth method to Password."
            ))
        }
    };
    let region = profile
        .region
        .clone()
        .unwrap_or_else(|| "us-east-1".to_string());

    let mut builder = AmazonS3Builder::new()
        .with_bucket_name(&bucket)
        .with_access_key_id(&access_key)
        .with_secret_access_key(&secret_key)
        .with_region(&region);

    if let Some(endpoint) = profile.endpoint.as_ref().filter(|s| !s.is_empty()) {
        builder = builder.with_endpoint(endpoint).with_allow_http(false);
    }
    // R2 and B2 want path-style addressing; AWS S3 supports both. Forcing
    // path-style is the safest default for custom endpoints.
    if profile.endpoint.as_ref().is_some_and(|s| !s.is_empty()) {
        builder = builder.with_virtual_hosted_style_request(false);
    }

    let store = builder
        .build()
        .with_context(|| format!("constructing S3 client for bucket {bucket}"))?;

    Ok(S3Session {
        id: Uuid::new_v4().to_string(),
        profile: profile.clone(),
        bucket,
        store: Arc::new(store),
    })
}
