use std::{
    fs::{File, OpenOptions},
    os::unix::prelude::PermissionsExt,
    path::Path,
};



pub fn create_writer(path: &Path) -> Result<File, std::io::Error> {
    if !path.parent().expect("Failed to get parent").exists() {
        std::fs::create_dir_all(path.parent().expect("Failed to get parent"))?;
    }

    let f = OpenOptions::new().append(true).create(true).open(path)?;

    let metadata = f.metadata()?;
    let mut permissions = metadata.permissions();

    permissions.set_mode(0o777);
    assert_eq!(permissions.mode(), 0o777);

    f.set_permissions(permissions)?;

    Ok(f)
}
