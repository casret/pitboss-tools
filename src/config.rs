use std::{env, fs};

use anyhow::{Context, Result};

/// Load the per-grill command password without ever printing it.
pub fn grill_password() -> Result<String> {
    if let Ok(password) = env::var("PITBOSS_GRILL_PASSWORD") {
        return Ok(password);
    }

    let mut paths = Vec::new();
    if let Ok(current_dir) = env::current_dir() {
        paths.push(current_dir.join("conf.toml"));
    }
    if let Ok(executable) = env::current_exe()
        && let Some(parent) = executable.parent()
    {
        let path = parent.join("conf.toml");
        if !paths.contains(&path) {
            paths.push(path);
        }
    }

    for path in paths {
        if !path.is_file() {
            continue;
        }
        let contents = fs::read_to_string(&path)
            .with_context(|| format!("could not read configuration file {}", path.display()))?;
        let document: toml::Table = toml::from_str(&contents)
            .with_context(|| format!("could not parse configuration file {}", path.display()))?;
        if let Some(password) = document
            .get("grill_password")
            .or_else(|| {
                document
                    .get("pitboss")
                    .and_then(|value| value.get("grill_password"))
            })
            .and_then(toml::Value::as_str)
        {
            return Ok(password.to_owned());
        }
    }

    anyhow::bail!(
        "no grill_password configured; set PITBOSS_GRILL_PASSWORD or add grill_password to conf.toml"
    )
}
