use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Scope {
    Automatic,
    Flat,
    Hierarchical,
    Global,
    StrictHierarchical,
}

#[derive(Debug, Clone)]
pub struct Project {
    pub documents: Vec<PathBuf>,
    pub scope: Scope,
    pub allow_port_net_names: bool,
    pub power_port_names_take_priority: bool,
    pub append_sheet_number_to_local_nets: bool,
    pub channel_designator_format: String,
}

impl Project {
    pub fn single(sheet: &Path) -> Project {
        Project {
            documents: vec![sheet.to_path_buf()],
            scope: Scope::Automatic,
            allow_port_net_names: false,
            power_port_names_take_priority: false,
            append_sheet_number_to_local_nets: false,
            channel_designator_format: "$Component_$RoomName".into(),
        }
    }
}

pub fn load(path: &Path) -> Result<Project, String> {
    let raw = std::fs::read(path).map_err(|e| format!("{}: {e}", path.display()))?;
    let text = String::from_utf8_lossy(&raw);
    let text = text.trim_start_matches('\u{feff}');
    let dir = path.parent().unwrap_or(Path::new(""));
    let mut project = Project::single(path);
    project.documents.clear();
    let mut section = String::new();
    for line in text.lines() {
        let line = line.trim();
        if let Some(s) = line.strip_prefix('[').and_then(|l| l.strip_suffix(']')) {
            section = s.to_ascii_lowercase();
            continue;
        }
        let Some((key, value)) = line.split_once('=') else {
            continue;
        };
        let on = value.trim() == "1";
        match (section.as_str(), key.trim()) {
            ("design", "HierarchyMode") => {
                project.scope = match value.trim() {
                    "1" => Scope::Flat,
                    "2" => Scope::Hierarchical,
                    "3" => Scope::Global,
                    "4" => Scope::StrictHierarchical,
                    _ => Scope::Automatic,
                }
            }
            ("design", "AllowPortNetNames") => project.allow_port_net_names = on,
            ("design", "PowerPortNamesTakePriority") => project.power_port_names_take_priority = on,
            ("design", "AppendSheetNumberToLocalNets") => project.append_sheet_number_to_local_nets = on,
            ("design", "ChannelDesignatorFormatString") if !value.trim().is_empty() => {
                project.channel_designator_format = value.trim().to_string();
            }
            (s, "DocumentPath") if s.starts_with("document") => {
                let rel = value.split('|').next().unwrap_or("").trim().replace('\\', "/");
                if rel.to_ascii_lowercase().ends_with(".schdoc") {
                    project.documents.push(dir.join(rel));
                }
            }
            _ => {}
        }
    }
    if project.documents.is_empty() {
        return Err(format!("{}: the project lists no schematic documents", path.display()));
    }
    Ok(project)
}
