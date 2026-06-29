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
