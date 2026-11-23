//! The fields of an SSH connection (name, host, user, port, key file, jump host), shared by
//! "Add SSH connection" and "Change".

use std::path::{Path, PathBuf};

use gpui_kit::component::input::{Input, InputState};
use gpui_kit::component::{ActiveTheme as _, Sizable as _, h_flex, v_flex};
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use remote::{ConfigValues, Target};

use crate::config::AppConfig;

/// What the fields say.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct HostFields {
    pub name: String,
    pub host: String,
    pub user: String,
    pub port: String,
    pub identity: String,
    pub jump: String,
}

/// A `Host` in the user's ssh config that a connection uses.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConfigEntry {
    pub alias: String,
    pub values: ConfigValues,
}

impl HostFields {
    /// The fields of a saved connection. What it takes from `config` shows as its own values.
    pub fn of(label: &str, target: &Target, config: Option<&ConfigEntry>) -> Self {
        let config = config.map(|config| &config.values);
        let (host, user, port, identity, jump) = match target {
            Target::Alias { alias } => (alias, None, None, None, None),
            Target::Custom {
                host,
                user,
                port,
                identity_file,
                proxy_jump,
            } => (
                host,
                user.clone(),
                *port,
                identity_file
                    .as_ref()
                    .map(|file| file.to_string_lossy().into_owned()),
                proxy_jump.clone(),
            ),
        };
        Self {
            name: label.to_owned(),
            host: config.map_or_else(|| host.clone(), |config| config.hostname.clone()),
            user: user.or_else(|| config?.user.clone()).unwrap_or_default(),
            port: port
                .or_else(|| Some(config?.port))
                .map(|port| port.to_string())
                .unwrap_or_default(),
            identity: identity
                .or_else(|| config?.identity_file.clone())
                .unwrap_or_default(),
            jump: jump
                .or_else(|| config?.proxy_jump.clone())
                .unwrap_or_default(),
        }
    }

    /// The name and target to save, or what is wrong. While the host is still the one
    /// `config` names, the connection keeps using that entry of the ssh config (with all its
    /// other options) and only what differs is saved; an emptied field uses the entry's value.
    pub fn target(
        &self,
        config: Option<&ConfigEntry>,
        home: Option<&Path>,
    ) -> Result<(String, Target), SharedString> {
        let optional = |text: &str| {
            let text = text.trim();
            (!text.is_empty()).then(|| text.to_owned())
        };
        let host = self.host.trim().to_owned();
        if host.is_empty() {
            return Err("Enter a host name or IP address.".into());
        }
        // ssh would read a leading `-` as one of its own options.
        let starts_with_dash = [&self.host, &self.user, &self.jump]
            .into_iter()
            .any(|text| text.trim().starts_with('-'));
        if starts_with_dash || host.chars().any(char::is_whitespace) {
            return Err(
                "Host, user and jump host cannot start with “-” (and the host has no spaces)."
                    .into(),
            );
        }
        let port = match optional(&self.port).map(|port| port.parse::<u16>()) {
            None => None,
            Some(Ok(port)) if port > 0 => Some(port),
            Some(_) => return Err("The port must be a number between 1 and 65535.".into()),
        };
        let label = optional(&self.name).unwrap_or_else(|| host.clone());
        let user = optional(&self.user);
        let identity_file = optional(&self.identity).map(|path| expand_home(&path, home));
        let proxy_jump = optional(&self.jump);
        let Some(ConfigEntry { alias, values }) =
            config.filter(|config| [&config.values.hostname, &config.alias].contains(&&host))
        else {
            return Ok((
                label,
                Target::Custom {
                    host,
                    user,
                    port,
                    identity_file,
                    proxy_jump,
                },
            ));
        };
        let config_identity = values
            .identity_file
            .as_ref()
            .map(|path| expand_home(path, home));
        let user = user.filter(|user| Some(user) != values.user.as_ref());
        let port = port.filter(|port| *port != values.port);
        let identity_file = identity_file.filter(|file| Some(file) != config_identity.as_ref());
        let proxy_jump = proxy_jump.filter(|jump| Some(jump) != values.proxy_jump.as_ref());
        let alias = alias.clone();
        let same =
            user.is_none() && port.is_none() && identity_file.is_none() && proxy_jump.is_none();
        let target = if same {
            Target::Alias { alias }
        } else {
            Target::Custom {
                host: alias,
                user,
                port,
                identity_file,
                proxy_jump,
            }
        };
        Ok((label, target))
    }
}

pub struct HostForm {
    name: Entity<InputState>,
    host: Entity<InputState>,
    user: Entity<InputState>,
    port: Entity<InputState>,
    identity: Entity<InputState>,
    jump: Entity<InputState>,
    pub error: Option<SharedString>,
}

impl HostForm {
    /// With `config`, an emptied field says which value the ssh config gives it.
    pub fn new<T>(
        fields: &HostFields,
        config: Option<&ConfigEntry>,
        window: &mut Window,
        cx: &mut Context<T>,
    ) -> Self {
        let values = config.map(|config| &config.values);
        let from_config = |value: Option<String>, otherwise: &str| {
            value.map_or_else(
                || otherwise.to_owned(),
                |value| format!("{value} (from ~/.ssh/config)"),
            )
        };
        let user = from_config(values.and_then(|v| v.user.clone()), "User (optional)");
        let port = values.map_or_else(|| "Port (22)".to_owned(), |v| v.port.to_string());
        let identity = from_config(
            values.and_then(|v| v.identity_file.clone()),
            "Key file, e.g. ~/.ssh/id_ed25519 (optional)",
        );
        let jump = from_config(
            values.and_then(|v| v.proxy_jump.clone()),
            "Jump host, e.g. user@bastion (optional)",
        );
        let mut input = |placeholder: &str, value: &str| {
            let (placeholder, value) = (placeholder.to_owned(), value.to_owned());
            cx.new(|cx| {
                InputState::new(window, cx)
                    .placeholder(placeholder)
                    .default_value(value)
            })
        };
        Self {
            name: input("Name (e.g. Build server)", &fields.name),
            host: input("Host name or IP address", &fields.host),
            user: input(&user, &fields.user),
            port: input(&port, &fields.port),
            identity: input(&identity, &fields.identity),
            jump: input(&jump, &fields.jump),
            error: None,
        }
    }

    pub fn inputs(&self) -> [&Entity<InputState>; 6] {
        [
            &self.name,
            &self.host,
            &self.user,
            &self.port,
            &self.identity,
            &self.jump,
        ]
    }

    /// See [`HostFields::target`].
    pub fn target(
        &self,
        config: Option<&ConfigEntry>,
        cx: &App,
    ) -> Result<(String, Target), SharedString> {
        let value = |input: &Entity<InputState>| input.read(cx).value().to_string();
        let fields = HostFields {
            name: value(&self.name),
            host: value(&self.host),
            user: value(&self.user),
            port: value(&self.port),
            identity: value(&self.identity),
            jump: value(&self.jump),
        };
        fields.target(config, cx.global::<AppConfig>().home.as_deref())
    }

    pub fn render(&self, cx: &App) -> Div {
        v_flex()
            .gap_2()
            .child(field("Name", &self.name, cx))
            .child(field("Host", &self.host, cx))
            .child(
                h_flex()
                    .gap_2()
                    .child(div().flex_1().child(field("User", &self.user, cx)))
                    .child(div().w(px(90.)).child(field("Port", &self.port, cx))),
            )
            .child(field("Key file", &self.identity, cx))
            .child(field("Jump host", &self.jump, cx))
            .when_some(self.error.clone(), |this, error| {
                this.child(
                    div()
                        .text_xs()
                        .text_color(crate::theme::palette(cx).error)
                        .child(error),
                )
            })
    }
}

/// A labelled text field.
pub fn field(label: &'static str, input: &Entity<InputState>, cx: &App) -> Div {
    v_flex()
        .gap_1()
        .child(
            div()
                .text_xs()
                .text_color(cx.theme().muted_foreground)
                .child(label),
        )
        .child(Input::new(input).small())
}

/// `~/x` → `<home>/x`, so key paths work regardless of how ssh is invoked.
fn expand_home(path: &str, home: Option<&Path>) -> PathBuf {
    match (path.strip_prefix("~/"), home) {
        (Some(rest), Some(home)) => home.join(rest),
        _ => PathBuf::from(path),
    }
}

#[cfg(test)]
mod tests {
    use std::path::{Path, PathBuf};

    use pretty_assertions::assert_eq;
    use remote::{ConfigValues, Target};

    use super::{ConfigEntry, HostFields, expand_home};

    fn entry() -> ConfigEntry {
        ConfigEntry {
            alias: "box".into(),
            values: ConfigValues {
                hostname: "192.0.2.7".into(),
                user: Some("me".into()),
                port: 2222,
                identity_file: Some("~/.ssh/box".into()),
                proxy_jump: None,
            },
        }
    }

    fn alias() -> Target {
        Target::Alias {
            alias: "box".into(),
        }
    }

    fn target(fields: &HostFields, config: Option<&ConfigEntry>) -> Target {
        let (_, target) = fields.target(config, Some(Path::new("/home/u"))).unwrap();
        target
    }

    #[test]
    fn a_config_host_shows_its_values_and_stays_one_unless_changed() {
        let config = entry();
        let fields = HostFields::of("Box", &alias(), Some(&config));
        let expected = HostFields {
            name: "Box".into(),
            host: "192.0.2.7".into(),
            user: "me".into(),
            port: "2222".into(),
            identity: "~/.ssh/box".into(),
            jump: String::new(),
        };
        assert_eq!(fields, expected);
        assert_eq!(target(&fields, Some(&config)), alias());
        // An emptied field uses the config's value.
        let emptied = HostFields {
            user: String::new(),
            port: " ".into(),
            identity: String::new(),
            ..fields
        };
        assert_eq!(target(&emptied, Some(&config)), alias());
    }

    #[test]
    fn only_what_differs_from_the_config_is_saved() {
        let config = entry();
        let mut fields = HostFields::of("Box", &alias(), Some(&config));
        fields.user = "root".into();
        fields.jump = "bastion".into();
        let changed = target(&fields, Some(&config));
        let expected = Target::Custom {
            host: "box".into(),
            user: Some("root".into()),
            port: None,
            identity_file: None,
            proxy_jump: Some("bastion".into()),
        };
        assert_eq!(changed, expected);
        // And shows the same again.
        assert_eq!(HostFields::of("Box", &changed, Some(&config)), fields);
        // Back to the config's values: the plain entry again.
        fields.user = "me".into();
        fields.jump = String::new();
        assert_eq!(target(&fields, Some(&config)), alias());
    }

    #[test]
    fn another_host_keeps_every_field() {
        let config = entry();
        let mut fields = HostFields::of("Box", &alias(), Some(&config));
        fields.host = "other.example.com".into();
        let expected = Target::Custom {
            host: "other.example.com".into(),
            user: Some("me".into()),
            port: Some(2222),
            identity_file: Some("/home/u/.ssh/box".into()),
            proxy_jump: None,
        };
        assert_eq!(target(&fields, Some(&config)), expected);
        // Without a config entry the fields are the target.
        let shown = HostFields::of("Box", &expected, None);
        assert_eq!(target(&shown, None), expected);
    }

    #[test]
    fn rejects_what_ssh_would_misread() {
        let fields = |host: &str, user: &str, port: &str| HostFields {
            host: host.into(),
            user: user.into(),
            port: port.into(),
            ..HostFields::default()
        };
        for bad in [
            fields("", "", ""),
            fields("-oProxyCommand=x", "", ""),
            fields("a b", "", ""),
            fields("h", "-x", ""),
            fields("h", "", "0"),
            fields("h", "", "65536"),
            fields("h", "", "ssh"),
        ] {
            assert!(bad.target(None, None).is_err(), "{bad:?}");
        }
        let (label, _) = fields("h", "", "22").target(None, None).unwrap();
        assert_eq!(label, "h");
    }

    #[test]
    fn expands_home_in_key_paths() {
        assert_eq!(
            expand_home("~/.ssh/id", Some(Path::new("/home/u"))),
            PathBuf::from("/home/u/.ssh/id")
        );
        assert_eq!(
            expand_home("/abs/key", Some(Path::new("/home/u"))),
            PathBuf::from("/abs/key")
        );
        assert_eq!(expand_home("~/.ssh/id", None), PathBuf::from("~/.ssh/id"));
    }
}
