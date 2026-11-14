//! The mesh of a puppet: what the header says, the vertices, the triangles.

use super::PuppetError;
use super::bytes::Reader;

const POSITION: u32 = 0x1;
const NORMAL: u32 = 0x2;
const TANGENT: u32 = 0x4;
const UV: u32 = 0x8;
const BONES: u32 = 0x0080_0000;
const WEIGHTS: u32 = 0x0100_0000;
const KNOWN: u32 = POSITION | NORMAL | TANGENT | UV | BONES | WEIGHTS;

/// One corner of the mesh.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct PuppetVertex {
    /// Where it is in the layer: pixels from the image's centre, x right, y up.
    pub position: [f32; 3],
    /// Which point of the image it shows: `0 0` is the image's top left corner.
    pub uv: [f32; 2],
    /// The bones that move it; a slot with no weight names bone 0.
    pub bones: [u32; 4],
    /// How much each of those bones moves it; all zero in a mesh without a skeleton.
    pub weights: [f32; 4],
    /// The surface normal: `0 0 1` when the file has none.
    pub normal: [f32; 3],
    /// The tangent and its handedness: `1 0 0 1` when the file has none.
    pub tangent: [f32; 4],
}

/// A run of triangles that follow one bone the most.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Part {
    /// The bone.
    pub bone: u32,
    /// Where the run starts in [`Puppet::indices`](super::Puppet::indices).
    pub first_index: u32,
    /// How many indices it takes.
    pub index_count: u32,
}

/// A run of triangles drawn with one material: one of the meshes of a model.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Surface {
    /// The material's path.
    pub material: String,
    /// Where the run starts in [`Puppet::indices`](super::Puppet::indices).
    pub first_index: u32,
    /// How many indices it takes.
    pub index_count: u32,
}

/// What the start of the file holds: every mesh, put one after the other.
pub(super) struct Mesh {
    pub(super) vertices: Vec<PuppetVertex>,
    pub(super) indices: Vec<u32>,
    pub(super) parts: Vec<Part>,
    pub(super) surfaces: Vec<Surface>,
    /// Whether the vertices carry bones and weights.
    pub(super) skinned: bool,
}

/// More meshes than this in one file is a number that is not a count.
const MAX_MESHES: u32 = 4096;

/// What stands between the material's path and the vertices; it changed twice.
#[derive(Clone, Copy, PartialEq)]
enum Layout {
    /// Nothing: the vertices are laid out as the header's flags say.
    Bare,
    /// The vertex format.
    Format,
    /// The bounding box, then the vertex format.
    BoundsFormat,
}

impl Layout {
    fn of(version: u32) -> Self {
        match version {
            0..=13 => Layout::Bare,
            14..=16 => Layout::Format,
            _ => Layout::BoundsFormat,
        }
    }
}

/// Reads the file from its `MDLV` header to the end of the last mesh and whatever
/// follows it up to the first section.
///
/// The header's third number counts the meshes (one for a puppet, one a material for
/// a model). Each mesh is a material's path and its geometry, as the module's
/// documentation describes; the next one starts right after the last one's trailer.
pub(super) fn read(reader: &mut Reader, version: u32) -> Result<Mesh, PuppetError> {
    let flags = reader.u32()?;
    reader.take(4)?;
    let count = reader.u32()?;
    if count == 0 || count > MAX_MESHES {
        return Err(PuppetError::Corrupt("the number of meshes is not a count"));
    }
    let mut mesh = Mesh {
        vertices: Vec::new(),
        indices: Vec::new(),
        parts: Vec::new(),
        surfaces: Vec::new(),
        skinned: true,
    };
    for index in 0..count {
        let material = reader.string()?;
        reader.take(4)?;
        let last = index + 1 == count;
        let Mesh { vertices, indices, parts, skinned, .. } = one(reader, flags, version, last)?;
        let base =
            u32::try_from(mesh.vertices.len()).map_err(|_| PuppetError::Corrupt("too many vertices"))?;
        let first_index =
            u32::try_from(mesh.indices.len()).map_err(|_| PuppetError::Corrupt("too many triangles"))?;
        mesh.surfaces.push(Surface { material, first_index, index_count: indices.len() as u32 });
        mesh.parts.extend(
            parts.into_iter().map(|part| Part { first_index: part.first_index + first_index, ..part }),
        );
        mesh.indices.extend(indices.into_iter().map(|i| i + base));
        mesh.vertices.extend(vertices);
        // A model whose meshes disagree is drawn without bones.
        mesh.skinned &= skinned;
    }
    Ok(mesh)
}

/// One mesh's geometry and trailer, from after its material's path.
fn one(reader: &mut Reader, flags: u32, version: u32, last: bool) -> Result<Mesh, PuppetError> {
    let start = reader.clone();
    // Versions nobody has shown us may sit on either side of a layout change, so the
    // layout the version suggests is tried first and the others after it.
    let expected = Layout::of(version);
    let layouts = [expected, Layout::BoundsFormat, Layout::Format, Layout::Bare];
    let mut first_failure = None;
    for (attempt, layout) in layouts.into_iter().enumerate() {
        if attempt > 0 && layout == expected {
            continue;
        }
        *reader = start.clone();
        match geometry(reader, flags, layout) {
            Ok((vertices, indices, skinned)) => {
                let parts = trailer(reader, version, last);
                return Ok(Mesh { vertices, indices, parts, surfaces: Vec::new(), skinned });
            }
            Err(why) => first_failure = first_failure.or(Some(why)),
        }
    }
    Err(first_failure.unwrap_or(PuppetError::Truncated))
}

fn geometry(
    reader: &mut Reader,
    flags: u32,
    layout: Layout,
) -> Result<(Vec<PuppetVertex>, Vec<u32>, bool), PuppetError> {
    if layout == Layout::BoundsFormat {
        reader.take(24)?;
    }
    let format = if layout == Layout::Bare { flags } else { reader.u32()? };
    if format & POSITION == 0 || format & !KNOWN != 0 {
        return Err(PuppetError::Unknown(format!("vertex format {format:#x}")));
    }
    let stride = [(POSITION, 12), (NORMAL, 12), (TANGENT, 16), (BONES, 16), (WEIGHTS, 16), (UV, 8)]
        .iter()
        .filter(|(bit, _)| format & bit != 0)
        .map(|(_, size)| size)
        .sum::<usize>();

    let vertex_bytes = reader.u32()? as usize;
    if !vertex_bytes.is_multiple_of(stride) {
        return Err(PuppetError::Corrupt("the vertex data is not a whole number of vertices"));
    }
    let mut data = reader
        .until(reader.position().checked_add(vertex_bytes).ok_or(PuppetError::Truncated)?)
        .map_err(|_| PuppetError::Truncated)?;
    reader.take(vertex_bytes)?;
    let mut vertices = Vec::with_capacity(vertex_bytes / stride);
    while data.remaining() > 0 {
        let mut vertex = PuppetVertex {
            position: data.floats()?,
            uv: [0.0; 2],
            bones: [0; 4],
            weights: [0.0; 4],
            normal: [0.0, 0.0, 1.0],
            tangent: [1.0, 0.0, 0.0, 1.0],
        };
        if format & NORMAL != 0 {
            vertex.normal = data.floats()?;
        }
        if format & TANGENT != 0 {
            vertex.tangent = data.floats()?;
        }
        if format & BONES != 0 {
            for bone in &mut vertex.bones {
                *bone = data.u32()?;
            }
        }
        if format & WEIGHTS != 0 {
            vertex.weights = data.floats()?;
        }
        if format & UV != 0 {
            vertex.uv = data.floats()?;
        }
        vertices.push(vertex);
    }

    let index_bytes = reader.u32()? as usize;
    if !index_bytes.is_multiple_of(6) {
        return Err(PuppetError::Corrupt("the index data is not a whole number of triangles"));
    }
    let indices: Vec<u32> = reader
        .take(index_bytes)?
        .as_chunks::<2>()
        .0
        .iter()
        .map(|index| u32::from(u16::from_le_bytes(*index)))
        .collect();
    if indices.iter().any(|index| *index as usize >= vertices.len()) {
        return Err(PuppetError::Corrupt("a triangle names a vertex the mesh does not have"));
    }
    Ok((vertices, indices, format & BONES != 0 && format & WEIGHTS != 0))
}

/// Reads what newer versions put after a mesh's triangles, and leaves the reader at the
/// next mesh or, after the last, at the first section. Returns the parts when the file
/// lists them.
fn trailer(reader: &mut Reader, version: u32, last: bool) -> Vec<Part> {
    let after_triangles = reader.clone();
    let parts = if version >= 21 { parts(reader, version).unwrap_or_default() } else { Vec::new() };
    if last && reader.clone().tag().is_none() {
        // Not where a section starts: the block was not what we took it for, or the
        // version has one we do not know. The skeleton's header is still findable.
        *reader = after_triangles;
        if !reader.find_tag(b'S') {
            let _ = reader.seek(reader.position() + reader.remaining());
        }
    }
    parts
}

fn parts(reader: &mut Reader, version: u32) -> Result<Vec<Part>, PuppetError> {
    // A second set of vertex positions (morph targets, as far as we can tell): skipped.
    if reader.u8()? != 0 {
        for _ in 0..reader.u32()? {
            let size = reader.u32()? as usize;
            reader.take(size)?;
        }
    }
    let mut parts = Vec::new();
    if reader.u8()? != 0 {
        let size = reader.u32()? as usize;
        for [bone, _, first_index, index_count] in reader.take(size)?.as_chunks::<4>().0.as_chunks::<4>().0 {
            parts.push(Part {
                bone: u32::from_le_bytes(*bone),
                first_index: u32::from_le_bytes(*first_index),
                index_count: u32::from_le_bytes(*index_count),
            });
        }
    }
    if version >= 23 {
        reader.take(4)?;
    }
    Ok(parts)
}
