use std::{fs::File, os::unix::prelude::PermissionsExt, path::Path};

use csv::Writer;

pub fn create_writer(path: &Path) -> Result<Writer<File>, std::io::Error> {
    let f = File::create(path)?;
    let metadata = f.metadata()?;
    let mut permissions = metadata.permissions();

    permissions.set_mode(0o664);
    Ok(Writer::from_writer(f))
}
