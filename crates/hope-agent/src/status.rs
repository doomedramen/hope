use crate::{config, reporting::Acceptance};
use anyhow::{Context, Result};
use std::path::Path;

#[derive(Default)]
struct Required {
    activation: Option<String>,
    process: Option<u32>,
    json: bool,
}
impl Required {
    fn parse(arguments: &[String]) -> Result<Self> {
        let mut result = Self::default();
        let mut arguments = arguments.iter();
        while let Some(option) = arguments.next() {
            match option.as_str() {
                "--json" => result.json = true,
                "--activation-id" => {
                    let value = arguments.next().context("Missing activation identifier")?;
                    uuid::Uuid::parse_str(value).context("Invalid activation identifier")?;
                    result.activation = Some(value.clone());
                }
                "--service-pid" => {
                    let process = arguments
                        .next()
                        .context("Missing service process identifier")?
                        .parse::<u32>()?;
                    anyhow::ensure!(process > 0, "Service has no running process");
                    result.process = Some(process);
                }
                _ => {
                    anyhow::bail!("Use status [--json] [--activation-id UUID] [--service-pid PID]")
                }
            }
        }
        Ok(result)
    }
    fn verify(&self, marker: &Acceptance, identity: &str) -> Result<()> {
        anyhow::ensure!(
            marker.agent_id == identity,
            "Acceptance marker belongs to another identity"
        );
        anyhow::ensure!(
            marker.accepted_at.is_finite() && marker.collected_at.is_finite(),
            "Invalid acceptance timestamps"
        );
        uuid::Uuid::parse_str(&marker.report_id).context("Invalid accepted report identifier")?;
        if let Some(expected) = &self.activation {
            anyhow::ensure!(
                marker.operation == "run" && marker.activation_id.as_ref() == Some(expected),
                "The activated service has not sent an accepted report"
            );
        }
        if let Some(process) = self.process {
            anyhow::ensure!(
                marker.operation == "run" && marker.process_id == process,
                "Acceptance marker does not match the running service process"
            );
        }
        Ok(())
    }
}

pub async fn show(directory: &Path, arguments: &[String]) -> Result<()> {
    let required = Required::parse(arguments)?;
    let configuration = config::read(directory).await?;
    let marker: Acceptance = serde_json::from_slice(
        &config::read_bounded(&directory.join("last-acceptance.json"), 4096)
            .await
            .context("No accepted service report yet")?,
    )?;
    required.verify(&marker, &configuration.agent_id)?;
    if required.json {
        println!("{}", serde_json::to_string(&marker)?);
    } else {
        println!(
            "Accepted report {} from process {} at {}",
            marker.report_id, marker.process_id, marker.accepted_at
        );
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn service_confirmation_requires_identity_activation_and_current_process() {
        let activation = uuid::Uuid::new_v4().to_string();
        let required = Required::parse(&[
            "--activation-id".into(),
            activation.clone(),
            "--service-pid".into(),
            "42".into(),
        ])
        .expect("options");
        let mut marker = Acceptance {
            agent_id: "identity".into(),
            report_id: uuid::Uuid::new_v4().to_string(),
            run_id: uuid::Uuid::new_v4().to_string(),
            process_id: 42,
            operation: "run".into(),
            collected_at: 100.0,
            accepted_at: 101.0,
            activation_id: Some(activation),
        };
        required
            .verify(&marker, "identity")
            .expect("current service");
        assert!(required.verify(&marker, "other").is_err());
        marker.operation = "configure".into();
        assert!(required.verify(&marker, "identity").is_err());
        marker.operation = "run".into();
        marker.process_id = 41;
        assert!(required.verify(&marker, "identity").is_err());
        marker.process_id = 42;
        marker.activation_id = None;
        assert!(required.verify(&marker, "identity").is_err());
        assert!(Required::parse(&["--service-pid".into(), "0".into()]).is_err());
        assert!(Required::parse(&["--activation-id".into(), "invalid".into()]).is_err());
    }
}
