// SPDX-License-Identifier: MPL-2.0

#[derive(Debug, Clone, Eq, PartialEq)]
pub(super) struct CheckOptions {
    pub(super) component: Component,
    pub(super) pool: Option<String>,
    pub(super) strict: bool,
    pub(super) online: bool,
}

impl Default for CheckOptions {
    fn default() -> Self {
        Self {
            component: Component::Standalone,
            pool: None,
            strict: false,
            online: false,
        }
    }
}

#[derive(Debug, Clone, Copy, Eq, PartialEq)]
pub(super) enum Component {
    Standalone,
    Engine,
    Worker,
    Trigger,
}

impl Component {
    fn parse(raw: &str) -> Result<Self, String> {
        match raw {
            "standalone" => Ok(Self::Standalone),
            "engine" => Ok(Self::Engine),
            "worker" => Ok(Self::Worker),
            "trigger" => Ok(Self::Trigger),
            other => Err(format!(
                "invalid --component `{other}` (expected standalone, engine, worker, or trigger)"
            )),
        }
    }

    pub(super) fn as_str(self) -> &'static str {
        match self {
            Self::Standalone => "standalone",
            Self::Engine => "engine",
            Self::Worker => "worker",
            Self::Trigger => "trigger",
        }
    }
}

#[derive(Debug, Clone, Eq, PartialEq)]
pub(super) enum CheckAction {
    Run(CheckOptions),
    Help,
}

pub(super) fn parse_check_args(args: &[String]) -> Result<CheckAction, String> {
    if args.len() == 1 && matches!(args[0].as_str(), "-h" | "--help" | "help") {
        return Ok(CheckAction::Help);
    }
    let mut options = CheckOptions::default();
    let mut index = 0;
    while index < args.len() {
        let arg = &args[index];
        match arg.as_str() {
            "--component" => {
                let value = value_after(args, &mut index, "--component")?;
                options.component = Component::parse(value)?;
            }
            "--pool" => options.pool = Some(pool_value(value_after(args, &mut index, "--pool")?)?),
            "--strict" => options.strict = true,
            "--online" => options.online = true,
            "-h" | "--help" | "help" => return Err(format!("unexpected argument `{arg}`")),
            other if other.starts_with("--component=") => {
                let value = other.trim_start_matches("--component=");
                options.component = Component::parse(value)?;
            }
            other if other.starts_with("--pool=") => {
                options.pool = Some(pool_value(other.trim_start_matches("--pool="))?);
            }
            other => return Err(format!("unexpected argument `{other}`")),
        }
        index += 1;
    }
    if options.pool.is_some() && options.component != Component::Worker {
        return Err("--pool is only valid with --component worker".to_string());
    }
    Ok(CheckAction::Run(options))
}

fn value_after<'a>(args: &'a [String], index: &mut usize, flag: &str) -> Result<&'a str, String> {
    *index += 1;
    args.get(*index)
        .filter(|value| !value.starts_with('-'))
        .map(String::as_str)
        .ok_or_else(|| format!("{flag} requires a value"))
}

fn pool_value(raw: &str) -> Result<String, String> {
    let value = raw.trim();
    if value.is_empty() {
        Err("--pool requires a non-empty value".to_string())
    } else {
        Ok(value.to_string())
    }
}
