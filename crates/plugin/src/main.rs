//! Standalone build of the plugin (JACK or the dummy backend), for
//! developing the editor and for the screenshot check.

fn main() {
    nice_plug::prelude::nice_export_standalone::<mesh_vst::MeshVst>();
}
