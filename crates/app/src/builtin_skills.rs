//! Skills that come with Pistachio. They are written into `<agent dir>/pistachio/skills` (only
//! when they changed) and handed to pi with `--skill` when a Code session starts on this
//! computer, so they load alongside the user's own skills without touching pi's skill folders.

use std::fs;
use std::io;
use std::path::{Path, PathBuf};

/// Each skill's name (its folder) and its `SKILL.md`.
const SKILLS: [(&str, &str); 1] = [(
    "add-skill",
    include_str!("builtin_skills/add-skill/SKILL.md"),
)];

/// Writes the skills into `dir` and returns their folders, for `--skill`.
pub fn install(dir: &Path) -> io::Result<Vec<PathBuf>> {
    SKILLS
        .iter()
        .map(|(name, text)| {
            let folder = dir.join(name);
            fs::create_dir_all(&folder)?;
            let path = folder.join("SKILL.md");
            if fs::read_to_string(&path).ok().as_deref() != Some(*text) {
                fs::write(&path, text)?;
            }
            Ok(folder)
        })
        .collect()
}

/// Whether the skill file at `path` is one of these (so the Skills page shows it read-only).
pub fn is_builtin(dir: &Path, path: &str) -> bool {
    Path::new(path).starts_with(dir)
}

#[cfg(test)]
mod tests {
    use pi_config::skills;

    #[test]
    fn skills_have_valid_front_matter_matching_their_folder() {
        for (name, text) in super::SKILLS {
            skills::validate_skill_file(text).unwrap();
            let front = skills::parse_front_matter(text);
            assert_eq!(front.name.as_deref(), Some(name));
            skills::validate_name(name).unwrap();
            skills::validate_description(&front.description.unwrap()).unwrap();
        }
    }

    #[test]
    fn install_writes_each_skill_once() {
        let dir = std::env::temp_dir().join(format!("pistachio-builtin-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let folders = super::install(&dir).unwrap();
        assert_eq!(folders, [dir.join("add-skill")]);
        let file = dir.join("add-skill/SKILL.md");
        assert_eq!(std::fs::read_to_string(&file).unwrap(), super::SKILLS[0].1);
        assert!(super::is_builtin(&dir, &file.to_string_lossy()));
        assert!(!super::is_builtin(&dir, "/elsewhere/add-skill/SKILL.md"));
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
