use super::table_rows;
use crate::BddWorld;
use anyhow::{Context as _, Result};
use cucumber::gherkin::Step;
use cucumber::given;
use std::fs::OpenOptions;
use std::io::Write as _;

#[given("a filesystem fixture with these entries")]
fn filesystem_fixture(world: &mut BddWorld, step: &Step) -> Result<()> {
    if world.fixture.is_none() {
        world.fixture = Some(tempfile::tempdir().context("create a filesystem fixture")?);
    }
    let root = world
        .fixture
        .as_ref()
        .context("a filesystem fixture exists")?
        .path();
    for row in table_rows(step, &["kind", "path", "value"])? {
        let [kind, path, value] = row.as_slice() else {
            anyhow::bail!("a fixture entry needs a kind, path and value, got {row:?}");
        };
        let path = root.join(path);
        let parent = path.parent().context("a fixture entry has a parent")?;
        std::fs::create_dir_all(parent).with_context(|| format!("create {}", parent.display()))?;
        match kind.as_str() {
            "file line" => {
                let mut file = OpenOptions::new()
                    .create(true)
                    .append(true)
                    .open(&path)
                    .with_context(|| format!("open {}", path.display()))?;
                writeln!(file, "{value}")
                    .with_context(|| format!("append a line to {}", path.display()))?;
            }
            "symlink" => std::os::unix::fs::symlink(value, &path)
                .with_context(|| format!("link {} to {value}", path.display()))?,
            _ => anyhow::bail!("unknown fixture entry kind {kind:?}"),
        }
    }
    Ok(())
}
