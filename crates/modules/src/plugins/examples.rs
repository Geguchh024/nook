//! The example plugins, built into Nook so they install offline from the
//! website's buttons (`nook://install?example=<id>`).

/// A plugin's files: (relative path, contents).
pub type Files = &'static [(&'static str, &'static [u8])];

/// (plugin id, its files).
pub static EXAMPLES: &[(&str, Files)] = &[
    (
        "counter",
        &[
            (
                "main.py",
                include_bytes!("../../../../examples/plugins/counter/main.py"),
            ),
            (
                "nook.py",
                include_bytes!("../../../../examples/plugins/counter/nook.py"),
            ),
            (
                "plugin.toml",
                include_bytes!("../../../../examples/plugins/counter/plugin.toml"),
            ),
        ],
    ),
    (
        "todo",
        &[
            (
                "main.py",
                include_bytes!("../../../../examples/plugins/todo/main.py"),
            ),
            (
                "nook.py",
                include_bytes!("../../../../examples/plugins/todo/nook.py"),
            ),
            (
                "plugin.toml",
                include_bytes!("../../../../examples/plugins/todo/plugin.toml"),
            ),
        ],
    ),
    (
        "disk-space",
        &[
            (
                "disk.ps1",
                include_bytes!("../../../../examples/plugins/disk-space/disk.ps1"),
            ),
            (
                "plugin.toml",
                include_bytes!("../../../../examples/plugins/disk-space/plugin.toml"),
            ),
        ],
    ),
    (
        "analog-clock",
        &[
            (
                "main.js",
                include_bytes!("../../../../examples/plugins/analog-clock/main.js"),
            ),
            (
                "nook.js",
                include_bytes!("../../../../examples/plugins/analog-clock/nook.js"),
            ),
            (
                "plugin.toml",
                include_bytes!("../../../../examples/plugins/analog-clock/plugin.toml"),
            ),
        ],
    ),
];

pub fn find(id: &str) -> Option<Files> {
    EXAMPLES
        .iter()
        .find(|(e, _)| *e == id)
        .map(|(_, files)| *files)
}
