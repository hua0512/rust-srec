use crate::credentials::CredentialOwner;
use crate::database::models::TemplateConfigDbModel;
use crate::database::repositories::{credential_selections, row_write::WriteMode};
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
            writes::write_template(connection, &cloned, WriteMode::Insert, cloned.updated_at.timestamp_millis()).await?;
            // Profiles belong to the platform, so the copy selects the same accounts.
            credential_selections::copy_owner(
                connection,
                &CredentialOwner::Template { template_id: source_id.clone() },
                &CredentialOwner::Template { template_id: cloned.id.clone() },
            )
            .await?;
            // The copy connects the same way.
            let route = super::proxies::route_of(connection, &super::RouteOwner::Template(source_id.clone())).await?;
            super::proxies::set_route(connection, &super::RouteOwner::Template(cloned.id.clone()), &route).await?;
            Ok(cloned)
        }), move |cloned| {
            if let Some(publication) = publication { publication(CredentialOwner::Template { template_id: cloned.id.clone() }); }
        }).await
    }
}
