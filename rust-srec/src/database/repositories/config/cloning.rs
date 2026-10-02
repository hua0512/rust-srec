use std::collections::HashMap;

use crate::credentials::{CredentialOwner, CredentialProfile, CredentialSelection};
use crate::database::models::TemplateConfigDbModel;
use crate::database::repositories::{credential_profiles, row_write::WriteMode};
use crate::{Error, Result};

use super::{SqlxConfigRepository, writes};

impl SqlxConfigRepository {
    pub(super) async fn clone_template_owned(
        &self,
        source_id: &str,
        new_name: &str,
    ) -> Result<TemplateConfigDbModel> {
        if new_name.trim().is_empty() {
            return Err(Error::validation("Template name cannot be empty"));
        }
        let source_id = source_id.to_owned();
        let new_name = new_name.to_owned();
        let publication = self.publication.get().cloned();
        self.writer.transaction("clone template configuration", move |connection| Box::pin(async move {
            let mut cloned = sqlx::query_as::<_, TemplateConfigDbModel>("SELECT * FROM template_config WHERE id = ? AND NOT EXISTS(SELECT 1 FROM retirement_config_deletions WHERE kind = 'template' AND config_id = template_config.id)")
                .bind(&source_id).fetch_optional(&mut *connection).await?.ok_or_else(|| Error::not_found("TemplateConfig", &source_id))?;
            cloned.id = uuid::Uuid::new_v4().to_string();
            cloned.name = new_name;
            cloned.created_at = chrono::Utc::now();
            cloned.updated_at = cloned.created_at;
            // Create the owner before its FK-bound profiles. No observer sees these
            // temporary source IDs: remapping and graph validation precede COMMIT.
            writes::write_template(connection, &cloned, WriteMode::Insert, cloned.updated_at.timestamp_millis()).await?;
            let profiles = sqlx::query_as::<_, CredentialProfile>("SELECT p.* FROM credential_profiles p WHERE p.template_id = ? AND NOT EXISTS(SELECT 1 FROM retirement_credential_profiles r WHERE r.profile_id = p.id) ORDER BY p.id")
                .bind(&source_id).fetch_all(&mut *connection).await?;
            let owner = CredentialOwner::Template { template_id: cloned.id.clone() };
            let mut ids = HashMap::new();
            for profile in profiles {
                let copy = credential_profiles::create_in(connection, &owner, &profile.platform_config_id, &profile.label, profile.enabled, &profile.material()?).await?;
                ids.insert(profile.id, copy.id);
            }
            if !ids.is_empty() && let Some(raw) = &cloned.platform_overrides {
                let mut overrides: serde_json::Value = serde_json::from_str(raw)?;
                if let Some(platforms) = overrides.as_object_mut() {
                    for entry in platforms.values_mut() {
                        if let Some(policy) = entry.get_mut("credential_selection") {
                            let mut selection = CredentialSelection::from_value(policy.clone())?;
                            match &mut selection {
                                CredentialSelection::Fixed { credential_id } => {
                                    if let Some(id) = ids.get(credential_id) { *credential_id = id.clone(); }
                                }
                                CredentialSelection::Pool { credential_ids, .. } => {
                                    for id in credential_ids { if let Some(replacement) = ids.get(id) { *id = replacement.clone(); } }
                                }
                                CredentialSelection::Inherit | CredentialSelection::None => {}
                            }
                            *policy = serde_json::to_value(selection)?;
                        }
                    }
                }
                cloned.platform_overrides = Some(serde_json::to_string(&overrides)?);
            }
            writes::write_template(connection, &cloned, WriteMode::Update, cloned.updated_at.timestamp_millis()).await?;
            credential_profiles::validate_graph(connection).await?;
            Ok(cloned)
        }), move |cloned| {
            if let Some(publication) = publication { publication(CredentialOwner::Template { template_id: cloned.id.clone() }); }
        }).await
    }
}
