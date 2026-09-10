use crate::config::ProviderCommandConfig;

/// A config-driven provider wrapping a CLI command and its launch configuration.
pub struct GenericProvider {
    pub name: String,
    pub config: ProviderCommandConfig,
}

impl GenericProvider {
    pub fn command(&self) -> &str {
        &self.config.command
    }
}

/// Create a [`GenericProvider`] from a provider name and its config.
pub fn create_provider(name: &str, config: ProviderCommandConfig) -> GenericProvider {
    GenericProvider {
        name: name.to_string(),
        config,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn create_provider_exposes_command() {
        let config = ProviderCommandConfig {
            command: "echo".to_string(),
            ..Default::default()
        };
        let prov = create_provider("custom", config);
        assert_eq!(prov.name, "custom");
        assert_eq!(prov.command(), "echo");
    }
}

/// The one refusal every surface gives for a provider name that has no
/// `[providers]` block, so a palette command, a web route and a toast agree.
pub fn provider_not_configured(provider: &str) -> String {
    format!("Provider \"{provider}\" is not configured. Pick one of the configured providers.")
}

#[cfg(test)]
mod refusal_tests {
    use super::provider_not_configured;

    #[test]
    fn the_refusal_names_the_provider_and_the_way_forward() {
        assert_eq!(
            provider_not_configured("frobnicate"),
            "Provider \"frobnicate\" is not configured. Pick one of the configured providers."
        );
    }
}
