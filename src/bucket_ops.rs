//! Handlers for the bucket configuration operations: ACLs, ownership controls, Block Public
//! Access, policy, CORS, lifecycle, website and notification. `s3impl.rs` forwards the trait
//! methods here; everything is synchronous because nothing waits on I/O.

use std::sync::Arc;
use std::sync::atomic::Ordering;

use http::StatusCode;
use s3s::dto::*;
use s3s::{S3Error, S3ErrorCode, S3Request, S3Response, S3Result, s3_error};

use crate::acl::{Acl, AclArgs, acl_args, resolve};
use crate::bucket_config::{BucketConfig, Ownership, PublicAccessBlock, custom_error};
use crate::lifecycle::Lifecycle;
use crate::policy::Policy;
use crate::s3impl::FakeS3;
use crate::store::Version;

type Out<T> = S3Result<S3Response<T>>;

const DEFAULT_MIN_SIZE: &str = "all_storage_classes_128K";

fn malformed_xml() -> S3Error {
    s3_error!(
        MalformedXML,
        "The XML you provided was not well-formed or did not validate against our published schema"
    )
}

fn access_denied(msg: &'static str) -> S3Error {
    S3Error::with_message(S3ErrorCode::AccessDenied, msg)
}

/// The ACL a PutBucketAcl / PutObjectAcl request asks for: from the body or from the headers.
fn requested_acl(cfg: &BucketConfig, body: Option<AccessControlPolicy>, args: &AclArgs<'_>, bucket: bool) -> S3Result<Acl> {
    match body {
        Some(_) if !args.is_empty() => Err(s3_error!(UnexpectedContent, "This request does not support content")),
        Some(p) => {
            if !cfg.acls_enabled() {
                return Err(s3_error!(AccessControlListNotSupported, "The bucket does not allow ACLs"));
            }
            let acl = Acl::from_dto(p)?;
            if acl.is_public() && cfg.public_access_block.read().is_some_and(|b| b.block_public_acls) {
                return Err(access_denied("Access Denied"));
            }
            Ok(acl)
        }
        None if args.is_empty() => Err(s3_error!(MissingSecurityHeader, "Your request was missing a required header")),
        None => Ok(resolve(cfg, args, bucket)?.unwrap_or_else(Acl::private)),
    }
}

/// What a GET reports: ACLs that are disabled read as the default.
fn visible(cfg: &BucketConfig, acl: Option<&Acl>) -> Vec<Grant> {
    match acl {
        Some(a) if cfg.acls_enabled() => a.to_dto(),
        _ => Acl::private().to_dto(),
    }
}

impl FakeS3 {
    pub(crate) fn do_get_bucket_acl(&self, req: S3Request<GetBucketAclInput>) -> Out<GetBucketAclOutput> {
        let b = self.bucket(&req.input.bucket)?;
        let acl = b.config.acl.read().clone();
        Ok(S3Response::new(GetBucketAclOutput {
            grants: Some(visible(&b.config, Some(&acl))),
            owner: Some(Acl::owner()),
        }))
    }

    pub(crate) fn do_put_bucket_acl(&self, req: S3Request<PutBucketAclInput>) -> Out<PutBucketAclOutput> {
        let mut input = req.input;
        let b = self.bucket(&input.bucket)?;
        let body = input.access_control_policy.take();
        let acl = requested_acl(&b.config, body, &acl_args!(&input, bucket), true)?;
        *b.config.acl.write() = acl;
        Ok(S3Response::new(PutBucketAclOutput {}))
    }

    pub(crate) fn do_get_object_acl(&self, req: S3Request<GetObjectAclInput>) -> Out<GetObjectAclOutput> {
        let input = req.input;
        let b = self.bucket(&input.bucket)?;
        let (v, _) = self.version(&input.bucket, &input.key, input.version_id.as_deref())?;
        let o = v.obj.as_ref().expect("resolved versions are never delete markers");
        Ok(S3Response::new(GetObjectAclOutput {
            grants: Some(visible(&b.config, o.acl.as_deref())),
            owner: Some(Acl::owner()),
            ..Default::default()
        }))
    }

    pub(crate) fn do_put_object_acl(&self, req: S3Request<PutObjectAclInput>) -> Out<PutObjectAclOutput> {
        let mut input = req.input;
        let b = self.bucket(&input.bucket)?;
        let body = input.access_control_policy.take();
        let acl = requested_acl(&b.config, body, &acl_args!(&input), false)?;
        let mut objects = b.objects.write();
        let entry = objects.get_mut(&input.key).ok_or_else(|| s3_error!(NoSuchKey))?;
        let pos = match &input.version_id {
            Some(id) => entry
                .versions
                .iter()
                .rposition(|v| &v.id == id)
                .ok_or_else(|| s3_error!(NoSuchVersion))?,
            None => entry.versions.len() - 1,
        };
        let v = &entry.versions[pos];
        let Some(o) = &v.obj else {
            return Err(if input.version_id.is_some() {
                s3_error!(MethodNotAllowed)
            } else {
                s3_error!(NoSuchKey)
            });
        };
        let mut o = o.clone();
        o.acl = (acl != Acl::private()).then(|| Arc::new(acl));
        entry.versions[pos] = Arc::new(Version {
            id: v.id.clone(),
            obj: Some(o),
            last_modified: v.last_modified,
        });
        Ok(S3Response::new(PutObjectAclOutput::default()))
    }

    // ---- ownership controls and Block Public Access ----

    pub(crate) fn do_get_bucket_ownership_controls(
        &self,
        req: S3Request<GetBucketOwnershipControlsInput>,
    ) -> Out<GetBucketOwnershipControlsOutput> {
        let b = self.bucket(&req.input.bucket)?;
        let o = (*b.config.ownership.read())
            .ok_or_else(|| s3_error!(OwnershipControlsNotFoundError, "The bucket ownership controls were not found"))?;
        Ok(S3Response::new(GetBucketOwnershipControlsOutput {
            ownership_controls: Some(OwnershipControls {
                rules: vec![OwnershipControlsRule {
                    object_ownership: ObjectOwnership::from(o.as_str().to_owned()),
                }],
            }),
        }))
    }

    pub(crate) fn do_put_bucket_ownership_controls(
        &self,
        req: S3Request<PutBucketOwnershipControlsInput>,
    ) -> Out<PutBucketOwnershipControlsOutput> {
        let b = self.bucket(&req.input.bucket)?;
        let [rule] = req.input.ownership_controls.rules.as_slice() else {
            return Err(malformed_xml());
        };
        let o = Ownership::parse(rule.object_ownership.as_str()).ok_or_else(malformed_xml)?;
        *b.config.ownership.write() = Some(o);
        Ok(S3Response::new(PutBucketOwnershipControlsOutput {}))
    }

    pub(crate) fn do_delete_bucket_ownership_controls(
        &self,
        req: S3Request<DeleteBucketOwnershipControlsInput>,
    ) -> Out<DeleteBucketOwnershipControlsOutput> {
        let b = self.bucket(&req.input.bucket)?;
        *b.config.ownership.write() = None;
        Ok(S3Response::new(DeleteBucketOwnershipControlsOutput {}))
    }

    pub(crate) fn do_get_public_access_block(&self, req: S3Request<GetPublicAccessBlockInput>) -> Out<GetPublicAccessBlockOutput> {
        let b = self.bucket(&req.input.bucket)?;
        let p = (*b.config.public_access_block.read()).ok_or_else(|| {
            custom_error(
                "NoSuchPublicAccessBlockConfiguration",
                StatusCode::NOT_FOUND,
                "The public access block configuration was not found",
            )
        })?;
        Ok(S3Response::new(GetPublicAccessBlockOutput {
            public_access_block_configuration: Some(PublicAccessBlockConfiguration {
                block_public_acls: Some(p.block_public_acls),
                ignore_public_acls: Some(p.ignore_public_acls),
                block_public_policy: Some(p.block_public_policy),
                restrict_public_buckets: Some(p.restrict_public_buckets),
            }),
        }))
    }

    pub(crate) fn do_put_public_access_block(&self, req: S3Request<PutPublicAccessBlockInput>) -> Out<PutPublicAccessBlockOutput> {
        let b = self.bucket(&req.input.bucket)?;
        let c = req.input.public_access_block_configuration;
        if c.block_public_acls.is_none()
            && c.ignore_public_acls.is_none()
            && c.block_public_policy.is_none()
            && c.restrict_public_buckets.is_none()
        {
            return Err(malformed_xml());
        }
        *b.config.public_access_block.write() = Some(PublicAccessBlock {
            block_public_acls: c.block_public_acls.unwrap_or(false),
            ignore_public_acls: c.ignore_public_acls.unwrap_or(false),
            block_public_policy: c.block_public_policy.unwrap_or(false),
            restrict_public_buckets: c.restrict_public_buckets.unwrap_or(false),
        });
        Ok(S3Response::new(PutPublicAccessBlockOutput {}))
    }

    pub(crate) fn do_delete_public_access_block(&self, req: S3Request<DeletePublicAccessBlockInput>) -> Out<DeletePublicAccessBlockOutput> {
        let b = self.bucket(&req.input.bucket)?;
        *b.config.public_access_block.write() = None;
        Ok(S3Response::new(DeletePublicAccessBlockOutput {}))
    }

    // ---- policy ----

    pub(crate) fn do_put_bucket_policy(&self, req: S3Request<PutBucketPolicyInput>) -> Out<PutBucketPolicyOutput> {
        let input = req.input;
        let b = self.bucket(&input.bucket)?;
        let policy = Policy::parse(&input.bucket, &input.policy)?;
        if policy.public && b.config.public_access_block.read().is_some_and(|p| p.block_public_policy) {
            return Err(access_denied(
                "User is not authorized to perform: s3:PutBucketPolicy because public policies are blocked by the BlockPublicPolicy block public access setting.",
            ));
        }
        if policy.uses_source_ip {
            self.store.access.record_peers.store(true, Ordering::Relaxed);
        }
        *b.config.policy.write() = Some(Arc::new(policy));
        self.store.access.any_policy.store(true, Ordering::Relaxed);
        Ok(S3Response::new(PutBucketPolicyOutput {}))
    }

    fn policy_of(&self, bucket: &str) -> S3Result<Arc<Policy>> {
        let b = self.bucket(bucket)?;
        let p = b.config.policy.read().clone();
        p.ok_or_else(|| s3_error!(NoSuchBucketPolicy, "The bucket policy does not exist"))
    }

    pub(crate) fn do_get_bucket_policy(&self, req: S3Request<GetBucketPolicyInput>) -> Out<GetBucketPolicyOutput> {
        let p = self.policy_of(&req.input.bucket)?;
        Ok(S3Response::new(GetBucketPolicyOutput {
            policy: Some(p.json.clone()),
        }))
    }

    pub(crate) fn do_delete_bucket_policy(&self, req: S3Request<DeleteBucketPolicyInput>) -> Out<DeleteBucketPolicyOutput> {
        let b = self.bucket(&req.input.bucket)?;
        *b.config.policy.write() = None;
        Ok(S3Response::new(DeleteBucketPolicyOutput {}))
    }

    pub(crate) fn do_get_bucket_policy_status(&self, req: S3Request<GetBucketPolicyStatusInput>) -> Out<GetBucketPolicyStatusOutput> {
        let p = self.policy_of(&req.input.bucket)?;
        Ok(S3Response::new(GetBucketPolicyStatusOutput {
            policy_status: Some(PolicyStatus { is_public: Some(p.public) }),
        }))
    }

    // ---- CORS ----

    pub(crate) fn do_put_bucket_cors(&self, req: S3Request<PutBucketCorsInput>) -> Out<PutBucketCorsOutput> {
        let b = self.bucket(&req.input.bucket)?;
        crate::cors::validate(&req.input.cors_configuration)?;
        *b.config.cors.write() = Some(Arc::new(req.input.cors_configuration));
        Ok(S3Response::new(PutBucketCorsOutput {}))
    }

    pub(crate) fn do_get_bucket_cors(&self, req: S3Request<GetBucketCorsInput>) -> Out<GetBucketCorsOutput> {
        let b = self.bucket(&req.input.bucket)?;
        let c = b
            .config
            .cors
            .read()
            .clone()
            .ok_or_else(|| s3_error!(NoSuchCORSConfiguration, "The CORS configuration does not exist"))?;
        Ok(S3Response::new(GetBucketCorsOutput {
            cors_rules: Some(c.cors_rules.clone()),
        }))
    }

    pub(crate) fn do_delete_bucket_cors(&self, req: S3Request<DeleteBucketCorsInput>) -> Out<DeleteBucketCorsOutput> {
        let b = self.bucket(&req.input.bucket)?;
        *b.config.cors.write() = None;
        Ok(S3Response::new(DeleteBucketCorsOutput {}))
    }

    // ---- lifecycle ----

    pub(crate) fn do_put_bucket_lifecycle(
        &self,
        req: S3Request<PutBucketLifecycleConfigurationInput>,
    ) -> Out<PutBucketLifecycleConfigurationOutput> {
        let input = req.input;
        let b = self.bucket(&input.bucket)?;
        let config = input.lifecycle_configuration.ok_or_else(malformed_xml)?;
        if let Some(t) = &input.transition_default_minimum_object_size
            && !matches!(t.as_str(), DEFAULT_MIN_SIZE | "varies_by_storage_class")
        {
            return Err(s3_error!(InvalidArgument, "Invalid transition default minimum object size"));
        }
        let mut lc = Lifecycle::new(config)?;
        lc.transition_default_minimum_object_size = input.transition_default_minimum_object_size.clone();
        *b.config.lifecycle.write() = Some(Arc::new(lc));
        // Apply the rules at the next access, whatever day it is.
        b.config.lifecycle_day.store(i64::MIN, Ordering::Release);
        b.config.has_lifecycle.store(true, Ordering::Release);
        self.store.access.any_lifecycle.store(true, Ordering::Relaxed);
        Ok(S3Response::new(PutBucketLifecycleConfigurationOutput {
            transition_default_minimum_object_size: input
                .transition_default_minimum_object_size
                .or_else(|| Some(TransitionDefaultMinimumObjectSize::from(DEFAULT_MIN_SIZE.to_owned()))),
        }))
    }

    pub(crate) fn do_get_bucket_lifecycle(
        &self,
        req: S3Request<GetBucketLifecycleConfigurationInput>,
    ) -> Out<GetBucketLifecycleConfigurationOutput> {
        let b = self.bucket(&req.input.bucket)?;
        let lc = b
            .config
            .lifecycle
            .read()
            .clone()
            .ok_or_else(|| s3_error!(NoSuchLifecycleConfiguration, "The lifecycle configuration does not exist"))?;
        Ok(S3Response::new(GetBucketLifecycleConfigurationOutput {
            rules: Some(lc.config.rules.clone()),
            transition_default_minimum_object_size: lc
                .transition_default_minimum_object_size
                .clone()
                .or_else(|| Some(TransitionDefaultMinimumObjectSize::from(DEFAULT_MIN_SIZE.to_owned()))),
        }))
    }

    pub(crate) fn do_delete_bucket_lifecycle(&self, req: S3Request<DeleteBucketLifecycleInput>) -> Out<DeleteBucketLifecycleOutput> {
        let b = self.bucket(&req.input.bucket)?;
        b.config.has_lifecycle.store(false, Ordering::Release);
        *b.config.lifecycle.write() = None;
        Ok(S3Response::new(DeleteBucketLifecycleOutput {}))
    }

    // ---- website ----

    pub(crate) fn do_put_bucket_website(&self, req: S3Request<PutBucketWebsiteInput>) -> Out<PutBucketWebsiteOutput> {
        let b = self.bucket(&req.input.bucket)?;
        validate_website(&req.input.website_configuration)?;
        *b.config.website.write() = Some(req.input.website_configuration);
        Ok(S3Response::new(PutBucketWebsiteOutput {}))
    }

    pub(crate) fn do_get_bucket_website(&self, req: S3Request<GetBucketWebsiteInput>) -> Out<GetBucketWebsiteOutput> {
        let b = self.bucket(&req.input.bucket)?;
        let w = b.config.website.read().clone().ok_or_else(|| {
            s3_error!(
                NoSuchWebsiteConfiguration,
                "The specified bucket does not have a website configuration"
            )
        })?;
        Ok(S3Response::new(GetBucketWebsiteOutput {
            error_document: w.error_document,
            index_document: w.index_document,
            redirect_all_requests_to: w.redirect_all_requests_to,
            routing_rules: w.routing_rules,
        }))
    }

    pub(crate) fn do_delete_bucket_website(&self, req: S3Request<DeleteBucketWebsiteInput>) -> Out<DeleteBucketWebsiteOutput> {
        let b = self.bucket(&req.input.bucket)?;
        *b.config.website.write() = None;
        Ok(S3Response::new(DeleteBucketWebsiteOutput {}))
    }

    // ---- notification ----

    pub(crate) fn do_put_bucket_notification(
        &self,
        req: S3Request<PutBucketNotificationConfigurationInput>,
    ) -> Out<PutBucketNotificationConfigurationOutput> {
        let b = self.bucket(&req.input.bucket)?;
        validate_notification(&req.input.notification_configuration)?;
        *b.config.notification.write() = Some(req.input.notification_configuration);
        Ok(S3Response::new(PutBucketNotificationConfigurationOutput {}))
    }

    pub(crate) fn do_get_bucket_notification(
        &self,
        req: S3Request<GetBucketNotificationConfigurationInput>,
    ) -> Out<GetBucketNotificationConfigurationOutput> {
        let b = self.bucket(&req.input.bucket)?;
        let n = b.config.notification.read().clone().unwrap_or_default();
        Ok(S3Response::new(GetBucketNotificationConfigurationOutput {
            event_bridge_configuration: n.event_bridge_configuration,
            lambda_function_configurations: n.lambda_function_configurations,
            queue_configurations: n.queue_configurations,
            topic_configurations: n.topic_configurations,
        }))
    }
}

fn invalid_argument(msg: &'static str) -> S3Error {
    S3Error::with_message(S3ErrorCode::InvalidArgument, msg)
}

fn validate_website(w: &WebsiteConfiguration) -> S3Result<()> {
    if let Some(r) = &w.redirect_all_requests_to {
        if w.index_document.is_some() || w.error_document.is_some() || w.routing_rules.is_some() {
            return Err(invalid_argument(
                "RedirectAllRequestsTo cannot be provided in conjunction with other Routing Rules.",
            ));
        }
        if r.host_name.is_empty() {
            return Err(malformed_xml());
        }
        if r.protocol.as_ref().is_some_and(|p| !matches!(p.as_str(), "http" | "https")) {
            return Err(invalid_argument(
                "Invalid protocol, protocol can be http or https. If not defined the protocol will be selected automatically.",
            ));
        }
        return Ok(());
    }
    let Some(index) = &w.index_document else {
        return Err(malformed_xml());
    };
    if index.suffix.is_empty() || index.suffix.contains('/') {
        return Err(invalid_argument("The IndexDocument Suffix is not well formed"));
    }
    for rule in w.routing_rules.iter().flatten() {
        let r = &rule.redirect;
        if r.replace_key_prefix_with.is_some() && r.replace_key_with.is_some() {
            return Err(invalid_argument("You can only define ReplaceKeyPrefix or ReplaceKey but not both."));
        }
        if r.host_name.is_none()
            && r.http_redirect_code.is_none()
            && r.protocol.is_none()
            && r.replace_key_prefix_with.is_none()
            && r.replace_key_with.is_none()
        {
            return Err(malformed_xml());
        }
        let c = &rule.condition;
        if c.as_ref()
            .is_some_and(|c| c.http_error_code_returned_equals.is_none() && c.key_prefix_equals.is_none())
        {
            return Err(malformed_xml());
        }
    }
    Ok(())
}

fn validate_notification(n: &NotificationConfiguration) -> S3Result<()> {
    let arn = |s: &str| {
        if s.starts_with("arn:") {
            Ok(())
        } else {
            Err(invalid_argument("The ARN is not well formed"))
        }
    };
    for t in n.topic_configurations.iter().flatten() {
        arn(&t.topic_arn)?;
        if t.events.is_empty() {
            return Err(malformed_xml());
        }
    }
    for q in n.queue_configurations.iter().flatten() {
        arn(&q.queue_arn)?;
        if q.events.is_empty() {
            return Err(malformed_xml());
        }
    }
    for l in n.lambda_function_configurations.iter().flatten() {
        arn(&l.lambda_function_arn)?;
        if l.events.is_empty() {
            return Err(malformed_xml());
        }
    }
    Ok(())
}
