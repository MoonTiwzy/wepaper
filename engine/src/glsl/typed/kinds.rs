//! The kinds of values in a shader, how the original converts between them, and
//! what the built-in functions take and give.

/// What the numbers of a value are.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub(super) enum Base {
    Bool,
    Int,
    Uint,
    Float,
}

/// The kind of a value.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) enum Kind {
    /// A number or a vector of `width` numbers (1 to 4).
    Value(Base, u8),
    /// Columns and rows of floats.
    Matrix(u8, u8),
    /// A texture: how many coordinates it is read with, and whether it compares depth.
    Sampler(u8, bool),
    /// A list of values of one kind.
    Array(Box<Kind>),
    /// A struct the shader declares, by its name.
    Struct(String),
    Void,
}

/// The scalar of `base`.
pub(super) const fn scalar(base: Base) -> Kind {
    Kind::Value(base, 1)
}

pub(super) const FLOAT: Kind = scalar(Base::Float);
pub(super) const BOOL: Kind = scalar(Base::Bool);

impl Kind {
    /// The kind a GLSL word names: `vec3`, `uint`, `mat4x3`, `sampler2D`.
    pub fn named(word: &str) -> Option<Self> {
        let width = |digit: &str| digit.parse::<u8>().ok().filter(|w| (2..=4).contains(w));
        Some(match word {
            "float" => scalar(Base::Float),
            "int" => scalar(Base::Int),
            "uint" => scalar(Base::Uint),
            "bool" => scalar(Base::Bool),
            "void" => Self::Void,
            "sampler2D" | "isampler2D" | "usampler2D" => Self::Sampler(2, false),
            "sampler3D" | "samplerCube" | "sampler2DArray" => Self::Sampler(3, false),
            "sampler2DShadow" => Self::Sampler(3, true),
            _ => {
                if let Some(rest) = word.strip_prefix("mat") {
                    return match rest.split_once('x') {
                        Some((columns, rows)) => Some(Self::Matrix(width(columns)?, width(rows)?)),
                        None => width(rest).map(|side| Self::Matrix(side, side)),
                    };
                }
                let (base, rest) = match word.as_bytes().first()? {
                    b'v' => (Base::Float, word),
                    b'i' => (Base::Int, &word[1..]),
                    b'u' => (Base::Uint, &word[1..]),
                    b'b' => (Base::Bool, &word[1..]),
                    _ => return None,
                };
                Self::Value(base, width(rest.strip_prefix("vec")?)?)
            }
        })
    }

    /// The word GLSL has for this kind, where it has one a constructor can be called by.
    pub fn word(&self) -> Option<String> {
        Some(match *self {
            Self::Value(base, 1) => match base {
                Base::Bool => "bool",
                Base::Int => "int",
                Base::Uint => "uint",
                Base::Float => "float",
            }
            .to_owned(),
            Self::Value(base, width) => {
                let letter = match base {
                    Base::Bool => "b",
                    Base::Int => "i",
                    Base::Uint => "u",
                    Base::Float => "",
                };
                format!("{letter}vec{width}")
            }
            Self::Matrix(columns, rows) if columns == rows => format!("mat{columns}"),
            Self::Matrix(columns, rows) => format!("mat{columns}x{rows}"),
            Self::Struct(ref name) => name.clone(),
            _ => return None,
        })
    }

    /// The number and the width of a scalar or a vector.
    pub fn value(&self) -> Option<(Base, u8)> {
        match *self {
            Self::Value(base, width) => Some((base, width)),
            _ => None,
        }
    }
}

/// Whether GLSL 3.30 takes a value of the kind `from` where `to` is wanted, as it is:
/// the same kind, or whole numbers where fractions are wanted.
pub(super) fn implicit(from: &Kind, to: &Kind) -> bool {
    from == to
        || matches!((from, to), (Kind::Value(Base::Int | Base::Uint, a), Kind::Value(Base::Float, b)) if a == b)
}

/// Whether the original converts a value of the kind `from` to `to` by itself: any
/// number to another kind of number, a vector to a shorter one, a number spread over
/// a vector, a matrix to a smaller one. A constructor of `to` does the same in GLSL.
pub(super) fn converts(from: &Kind, to: &Kind) -> bool {
    match (from, to) {
        (Kind::Value(_, from), Kind::Value(_, to)) => from >= to || *from == 1,
        (Kind::Matrix(c, r), Kind::Matrix(columns, rows)) => c >= columns && r >= rows,
        _ => from == to,
    }
}

/// What a conversion from `from` to `to` costs, for choosing between the overloads of
/// a function: nothing when it is the same kind, little when GLSL takes it as it is,
/// more the more it changes. `None`: it does not convert.
pub(super) fn cost(from: &Kind, to: &Kind) -> Option<f32> {
    if from == to {
        return Some(0.0);
    }
    if implicit(from, to) {
        return Some(0.1);
    }
    if !converts(from, to) {
        return None;
    }
    Some(match (from.value(), to.value()) {
        (Some((a, from)), Some((b, to))) => {
            let other_numbers = if a == b { 0.0 } else { 0.25 };
            match from {
                _ if from == to => other_numbers,
                _ if from > to => f32::from(from - to) + other_numbers,
                _ => 0.5 + other_numbers,
            }
        }
        _ => 1.0,
    })
}

/// The kind two values of an arithmetic operation are taken as, the way the original
/// takes them: the larger kind of number (`bool` counts as `int`), and where both are
/// vectors the shorter length.
pub(super) fn common(a: (Base, u8), b: (Base, u8)) -> (Base, u8) {
    let base = a.0.max(b.0).max(Base::Int);
    let width = match (a.1, b.1) {
        (1, width) | (width, 1) => width,
        (a, b) => a.min(b),
    };
    (base, width)
}

/// The kind an operand of width `width` is taken as in an operation of kind
/// `(base, all)`: a number stays one, GLSL spreads it; a vector is all of the width.
pub(super) fn operand(base: Base, all: u8, width: u8) -> Kind {
    Kind::Value(base, if width == 1 { 1 } else { all })
}

/// What a built-in function takes and gives.
pub(super) struct Signature {
    /// For each argument, the kind it is taken as, where that is known.
    pub takes: Vec<Option<Kind>>,
    pub gives: Kind,
}

/// The functions that work a component at a time, with the arguments that may be a
/// number where the others are vectors.
const COMPONENTS: &[(&str, &[usize])] = &[
    ("radians", &[]),
    ("degrees", &[]),
    ("sin", &[]),
    ("cos", &[]),
    ("tan", &[]),
    ("asin", &[]),
    ("acos", &[]),
    ("atan", &[]),
    ("sinh", &[]),
    ("cosh", &[]),
    ("tanh", &[]),
    ("exp", &[]),
    ("log", &[]),
    ("exp2", &[]),
    ("log2", &[]),
    ("sqrt", &[]),
    ("inversesqrt", &[]),
    ("abs", &[]),
    ("sign", &[]),
    ("floor", &[]),
    ("ceil", &[]),
    ("trunc", &[]),
    ("round", &[]),
    ("roundEven", &[]),
    ("fract", &[]),
    ("dFdx", &[]),
    ("dFdy", &[]),
    ("fwidth", &[]),
    ("normalize", &[]),
    ("pow", &[]),
    ("mod", &[1]),
    ("min", &[1]),
    ("max", &[1]),
    ("clamp", &[1, 2]),
    ("mix", &[2]),
    ("step", &[0]),
    ("smoothstep", &[0, 1]),
    ("reflect", &[]),
    ("refract", &[2]),
    ("faceforward", &[]),
];

/// Of these, the ones that also take whole numbers.
const WHOLE: [&str; 5] = ["abs", "sign", "min", "max", "clamp"];

/// What the built-in function `name` takes and gives when it is called with values of
/// the kinds `given` (`None` where a kind is not known). `None`: not a built-in, or
/// not known well enough to say.
pub(super) fn builtin(name: &str, given: &[Option<Kind>]) -> Option<Signature> {
    let values = || given.iter().map(|kind| kind.as_ref().and_then(Kind::value)).collect::<Option<Vec<_>>>();
    // The width all vector arguments are taken at: the shortest.
    let shortest = |values: &[(Base, u8)]| values.iter().map(|v| v.1).filter(|w| *w > 1).min().unwrap_or(1);
    if let Some((_, numbers)) = COMPONENTS.iter().find(|(known, _)| *known == name) {
        let values = values()?;
        let width = shortest(&values);
        let whole = WHOLE.contains(&name) && values.iter().all(|v| matches!(v.0, Base::Int | Base::Uint));
        let base = if whole { values.iter().map(|v| v.0).max()? } else { Base::Float };
        let takes = values
            .iter()
            .enumerate()
            .map(|(at, value)| {
                let number = value.1 == 1 && numbers.contains(&at);
                Some(Kind::Value(base, if number { 1 } else { width }))
            })
            .collect();
        return Some(Signature { takes, gives: Kind::Value(base, width) });
    }
    let all = |kind: Kind| given.iter().map(|_| Some(kind.clone())).collect();
    match name {
        "length" => Some(Signature { takes: vec![None], gives: FLOAT }),
        "distance" | "dot" => {
            let width = shortest(&values()?);
            Some(Signature { takes: all(Kind::Value(Base::Float, width)), gives: FLOAT })
        }
        "cross" => {
            Some(Signature { takes: all(Kind::Value(Base::Float, 3)), gives: Kind::Value(Base::Float, 3) })
        }
        "any" | "all" => Some(Signature { takes: vec![None], gives: BOOL }),
        "lessThan" | "lessThanEqual" | "greaterThan" | "greaterThanEqual" | "equal" | "notEqual" => {
            let values = values()?;
            let width = shortest(&values);
            let base = values.iter().map(|v| v.0).max()?;
            Some(Signature { takes: all(Kind::Value(base, width)), gives: Kind::Value(Base::Bool, width) })
        }
        "texture" | "textureLod" | "textureGrad" | "texelFetch" | "textureSize" => texture(name, given),
        "transpose" | "inverse" | "matrixCompMult" => {
            Some(Signature { takes: vec![None; given.len()], gives: given.first()?.clone()? })
        }
        "determinant" => Some(Signature { takes: vec![None], gives: FLOAT }),
        "floatBitsToInt" | "floatBitsToUint" | "intBitsToFloat" | "uintBitsToFloat" | "isnan" | "isinf" => {
            let (_, width) = given.first()?.as_ref()?.value()?;
            let base = match name {
                "floatBitsToInt" => Base::Int,
                "floatBitsToUint" => Base::Uint,
                "isnan" | "isinf" => Base::Bool,
                _ => Base::Float,
            };
            Some(Signature { takes: vec![None], gives: Kind::Value(base, width) })
        }
        _ => None,
    }
}

/// The texture functions: the coordinates are as many as the sampler has, what
/// follows them is a number.
fn texture(name: &str, given: &[Option<Kind>]) -> Option<Signature> {
    let Some(Kind::Sampler(coordinates, shadow)) = given.first()?.clone() else { return None };
    let whole = name == "texelFetch";
    let base = if whole { Base::Int } else { Base::Float };
    let mut takes = vec![None, Some(Kind::Value(base, coordinates))];
    match name {
        "textureSize" => {
            return Some(Signature { takes: vec![None, None], gives: Kind::Value(Base::Int, 2) });
        }
        "textureGrad" => takes.extend([None, None]),
        "texelFetch" => takes.push(Some(scalar(Base::Int))),
        _ => takes.extend(given.iter().skip(2).map(|_| Some(FLOAT))),
    }
    (takes.len() == given.len())
        .then_some(Signature { takes, gives: if shadow { FLOAT } else { Kind::Value(Base::Float, 4) } })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn kinds_are_read_from_their_words_and_written_back() {
        for word in ["float", "uint", "bvec2", "ivec3", "vec4", "mat3", "mat4x3"] {
            assert_eq!(Kind::named(word).and_then(|k| k.word()).as_deref(), Some(word));
        }
        assert_eq!(Kind::named("mat4x3"), Some(Kind::Matrix(4, 3)));
        assert_eq!(Kind::named("vec5"), None);
        assert_eq!(Kind::named("value"), None);
    }

    #[test]
    fn the_original_cuts_and_spreads_where_glsl_takes_only_what_fits() {
        let k = |word: &str| Kind::named(word).expect("a kind");
        assert!(implicit(&k("ivec2"), &k("vec2")));
        assert!(!implicit(&k("int"), &k("uint")), "GLSL 3.30 has no conversion between these");
        assert!(converts(&k("vec4"), &k("vec2")) && converts(&k("float"), &k("vec3")));
        assert!(!converts(&k("vec2"), &k("vec3")));
        assert!(cost(&k("vec4"), &k("vec3")) < cost(&k("vec4"), &k("vec2")));
        assert_eq!(common((Base::Uint, 1), (Base::Int, 1)), (Base::Uint, 1));
        assert_eq!(common((Base::Float, 4), (Base::Float, 2)), (Base::Float, 2));
        assert_eq!(common((Base::Bool, 1), (Base::Float, 3)), (Base::Float, 3));
    }

    #[test]
    fn built_in_functions_take_their_vectors_at_one_length() {
        let k = |word: &str| Kind::named(word);
        let mix = builtin("mix", &[k("vec4"), k("vec3"), k("float")]).expect("mix");
        assert_eq!(mix.takes, [k("vec3"), k("vec3"), k("float")]);
        assert_eq!(mix.gives, Kind::Value(Base::Float, 3));
        let max = builtin("max", &[k("int"), k("int")]).expect("max");
        assert_eq!(max.gives, scalar(Base::Int));
        let sample = builtin("texture", &[Some(Kind::Sampler(2, false)), k("vec4")]).expect("texture");
        assert_eq!(sample.takes, [None, k("vec2")]);
        assert_eq!(sample.gives, Kind::Value(Base::Float, 4));
        assert!(builtin("mix", &[None, k("vec3"), k("float")]).is_none());
        assert!(builtin("someone", &[]).is_none());
    }
}
