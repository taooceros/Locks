use std::{fs::{File, OpenOptions}, os::unix::prelude::PermissionsExt, path::Path};

use csv::Writer;

pub fn create_writer(path: &Path) -> Result<Writer<File>, std::io::Error> {
    let f = OpenOptions::new().write(true)
        .create_new(true)
        .open(path)?;

    let metadata = f.metadata()?;
    let mut permissions = metadata.permissions();

    permissions.set_mode(0o777);
    assert_eq!(permissions.mode(), 0o777);

    f.set_permissions(permissions)?;

    Ok(Writer::from_writer(f))
}
