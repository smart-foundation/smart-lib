//! The shared value types domains actually reference in the contract.
//!
//! The set is **measured, not invented**: each wrapper here exists because the
//! canonical schemas reference the corresponding `T.*` struct (reference counts
//! as of the 2026-10-11 contract, in parentheses). Nothing the contract lacks
//! is added, and every wrapper is a thin `Copy`/`Clone` value with total
//! conversions to and from the generated capnp type.
//!
//! | Wrapper | Contract type | References tree-wide |
//! |---|---|---|
//! | [`Vec2`] | `T.Vec2` | 56 |
//! | [`Vec2i`] | `T.Vec2i` | 30 |
//! | [`Vec3`] | `T.Vec3` | 6 |
//! | [`Vec4`] | `T.Vec4` | 1 |
//! | [`Quaternion`] | `T.Quaternion` | via `T.SpatialPose` |
//! | [`SpatialPose`] | `T.SpatialPose` | 2 |
//! | [`Rect`] | `T.Rect` | 26 |
//! | [`Point`] | `T.Point` | 5 |
//! | [`Point3D`] | `T.Point3D` | 1 |
//! | [`Size`] | `T.Size` | 10 |
//! | [`CornerRadius`] | `T.CornerRadius` | 6 |
//! | [`FrameRate`] | `T.FrameRate` | 4 |
//! | [`Region`] | `T.Region` | 3 |
//! | [`Version`] | `T.Version` | 104 |
//! | [`Duration`] | `T.Duration` | 416 |
//! | [`Timestamp`] | `T.Timestamp` | 625 |
//! | [`MonotonicTimestamp`] | `T.MonotonicTimestamp` | 16 |
//!
//! Deliberately **not** wrapped (measured: zero or text-only references, or the
//! type is a transport-seam concern owned by a later wave): `T.Insets`,
//! `T.Transform2D`, `T.Rect3D`, `T.DataHandle`, `T.StringMap`,
//! `T.CapabilityHandle` (an ID — see [`crate::core::ids`]).

use alloc::string::String;
use alloc::vec::Vec;

use crate::schema::types_capnp;

macro_rules! plain_value {
    ($(#[$doc:meta])* $name:ident, $module:ident { $($field:ident : $ty:ty => $get:ident, $set:ident ;)* }) => {
        $(#[$doc])*
        #[derive(Copy, Clone, Debug, Default, PartialEq)]
        pub struct $name {
            $(
                #[doc = concat!("The `", stringify!($field), "` field (contract: `T.", stringify!($name), ".", stringify!($field), "`).")]
                pub $field: $ty
            ),*
        }

        impl $name {
            /// Construct a value.
            pub const fn new($($field: $ty),*) -> Self {
                $name { $($field),* }
            }

            /// Read from the generated capnp reader.
            pub fn from_reader(reader: &types_capnp::$module::Reader<'_>) -> Self {
                $name {
                    $($field: reader.$get()),*
                }
            }

            /// Write into the generated capnp builder.
            pub fn write_to(&self, builder: &mut types_capnp::$module::Builder<'_>) {
                $(builder.$set(self.$field);)*
            }
        }

        impl From<&types_capnp::$module::Reader<'_>> for $name {
            fn from(reader: &types_capnp::$module::Reader<'_>) -> Self {
                $name::from_reader(reader)
            }
        }

        impl From<&types_capnp::$module::Builder<'_>> for $name {
            fn from(builder: &types_capnp::$module::Builder<'_>) -> Self {
                $name::from_reader(&builder.reborrow_as_reader())
            }
        }
    };
}

plain_value! {
    /// 2-D geometric vector (continuous; origin arbitrary). Contract: `T.Vec2`.
    Vec2, vec2 { x: f32 => get_x, set_x; y: f32 => get_y, set_y; }
}
plain_value! {
    /// 2-D integer vector. Contract: `T.Vec2i`.
    Vec2i, vec2i { x: i32 => get_x, set_x; y: i32 => get_y, set_y; }
}
plain_value! {
    /// 3-D geometric vector. Contract: `T.Vec3`.
    Vec3, vec3 { x: f32 => get_x, set_x; y: f32 => get_y, set_y; z: f32 => get_z, set_z; }
}
plain_value! {
    /// 4-D vector. Contract: `T.Vec4`.
    Vec4, vec4 { x: f32 => get_x, set_x; y: f32 => get_y, set_y; z: f32 => get_z, set_z; w: f32 => get_w, set_w; }
}
plain_value! {
    /// Unit quaternion (normalisation is the producer's job). Contract:
    /// `T.Quaternion`, reached through `T.SpatialPose`.
    Quaternion, quaternion { x: f32 => get_x, set_x; y: f32 => get_y, set_y; z: f32 => get_z, set_z; w: f32 => get_w, set_w; }
}
plain_value! {
    /// Pixel coordinate, origin top-left (discrete — not a `Vec2`). Contract:
    /// `T.Point`.
    Point, point { x: i32 => get_x, set_x; y: i32 => get_y, set_y; }
}
plain_value! {
    /// 3-D point. Contract: `T.Point3D`.
    Point3D, point3_d { x: f32 => get_x, set_x; y: f32 => get_y, set_y; z: f32 => get_z, set_z; }
}
plain_value! {
    /// Pixel size. Contract: `T.Size`.
    Size, size { width: u32 => get_width, set_width; height: u32 => get_height, set_height; }
}
plain_value! {
    /// Per-corner radii. Contract: `T.CornerRadius`.
    CornerRadius, corner_radius {
        top_left: f32 => get_top_left, set_top_left;
        top_right: f32 => get_top_right, set_top_right;
        bottom_right: f32 => get_bottom_right, set_bottom_right;
        bottom_left: f32 => get_bottom_left, set_bottom_left;
    }
}
plain_value! {
    /// Fractional frame rate (`numerator/denominator`). Contract: `T.FrameRate`.
    FrameRate, frame_rate { numerator: u32 => get_numerator, set_numerator; denominator: u32 => get_denominator, set_denominator; }
}
plain_value! {
    /// Rectangle with an explicit degenerate flag — a zero-area rect at a
    /// position is *not* the same as an empty region (`types.capnp:958`).
    /// Contract: `T.Rect`.
    Rect, rect {
        x: i32 => get_x, set_x;
        y: i32 => get_y, set_y;
        width: u32 => get_width, set_width;
        height: u32 => get_height, set_height;
        degenerate: bool => get_degenerate, set_degenerate;
    }
}

/// A set of rectangles. Contract: `T.Region`.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Region {
    /// The rectangles, in the producer's order.
    pub rectangles: Vec<Rect>,
}

impl Region {
    /// Read from the generated capnp reader.
    pub fn from_reader(reader: &types_capnp::region::Reader<'_>) -> Self {
        let rectangles = reader
            .get_rectangles()
            .map(|list| list.iter().map(|r| Rect::from_reader(&r)).collect())
            .unwrap_or_default();
        Region { rectangles }
    }

    /// Write into the generated capnp builder.
    pub fn write_to(&self, builder: &mut types_capnp::region::Builder<'_>) {
        let mut list = builder.reborrow().init_rectangles(self.rectangles.len() as u32);
        for (i, rect) in self.rectangles.iter().enumerate() {
            rect.write_to(&mut list.reborrow().get(i as u32));
        }
    }
}

impl From<&types_capnp::region::Reader<'_>> for Region {
    fn from(reader: &types_capnp::region::Reader<'_>) -> Self {
        Region::from_reader(reader)
    }
}

impl From<&types_capnp::region::Builder<'_>> for Region {
    fn from(builder: &types_capnp::region::Builder<'_>) -> Self {
        Region::from_reader(&builder.reborrow_as_reader())
    }
}

/// Where a thing is, which way it faces, how it moves. Contract:
/// `T.SpatialPose` — the one spatial pose shared by location, input and
/// anything else addressing space.
#[derive(Copy, Clone, Debug, Default, PartialEq)]
pub struct SpatialPose {
    /// Position.
    pub position: Vec3,
    /// Orientation.
    pub rotation: Quaternion,
    /// Linear velocity (m/s). Zero when unmeasured — that is an honest zero.
    pub velocity: Vec3,
    /// Angular velocity (rad/s). Zero when unmeasured.
    pub angular_velocity: Vec3,
}

impl SpatialPose {
    /// Read from the generated capnp reader.
    pub fn from_reader(reader: &types_capnp::spatial_pose::Reader<'_>) -> Self {
        // Struct-typed fields are pointer fields on the wire, so the generated
        // getters are fallible; an absent sub-struct reads as the zero value,
        // which is the contract's own default.
        SpatialPose {
            position: match reader.get_position() {
                Ok(p) => Vec3::from_reader(&p),
                Err(_) => Vec3::default(),
            },
            rotation: match reader.get_rotation() {
                Ok(q) => Quaternion::from_reader(&q),
                Err(_) => Quaternion::default(),
            },
            velocity: match reader.get_velocity() {
                Ok(v) => Vec3::from_reader(&v),
                Err(_) => Vec3::default(),
            },
            angular_velocity: match reader.get_angular_velocity() {
                Ok(v) => Vec3::from_reader(&v),
                Err(_) => Vec3::default(),
            },
        }
    }

    /// Write into the generated capnp builder.
    pub fn write_to(&self, builder: &mut types_capnp::spatial_pose::Builder<'_>) {
        self.position.write_to(&mut builder.reborrow().init_position());
        self.rotation.write_to(&mut builder.reborrow().init_rotation());
        self.velocity
            .write_to(&mut builder.reborrow().init_velocity());
        self.angular_velocity
            .write_to(&mut builder.reborrow().init_angular_velocity());
    }
}

impl From<&types_capnp::spatial_pose::Reader<'_>> for SpatialPose {
    fn from(reader: &types_capnp::spatial_pose::Reader<'_>) -> Self {
        SpatialPose::from_reader(reader)
    }
}

impl From<&types_capnp::spatial_pose::Builder<'_>> for SpatialPose {
    fn from(builder: &types_capnp::spatial_pose::Builder<'_>) -> Self {
        SpatialPose::from_reader(&builder.reborrow_as_reader())
    }
}

/// Monotonic time, immune to clock adjustment. Contract: `T.Duration`.
#[derive(Copy, Clone, Debug, Default, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Duration {
    /// Nanoseconds.
    pub nanoseconds: u64,
}

impl Duration {
    /// The zero duration.
    pub const ZERO: Duration = Duration { nanoseconds: 0 };

    /// From nanoseconds (the contract's only unit).
    pub const fn from_nanos(nanoseconds: u64) -> Self {
        Duration { nanoseconds }
    }

    /// As nanoseconds.
    pub const fn as_nanos(self) -> u64 {
        self.nanoseconds
    }

    /// Read from the generated capnp reader.
    pub fn from_reader(reader: &types_capnp::duration::Reader<'_>) -> Self {
        Duration {
            nanoseconds: reader.get_nanoseconds(),
        }
    }

    /// Write into the generated capnp builder.
    pub fn write_to(&self, builder: &mut types_capnp::duration::Builder<'_>) {
        builder.set_nanoseconds(self.nanoseconds);
    }
}

impl From<&types_capnp::duration::Reader<'_>> for Duration {
    fn from(reader: &types_capnp::duration::Reader<'_>) -> Self {
        Duration::from_reader(reader)
    }
}

impl From<&types_capnp::duration::Builder<'_>> for Duration {
    fn from(builder: &types_capnp::duration::Builder<'_>) -> Self {
        Duration::from_reader(&builder.reborrow_as_reader())
    }
}

/// Wall-clock time. Contract: `T.Timestamp`.
#[derive(Copy, Clone, Debug, Default, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Timestamp {
    /// Nanoseconds since the Unix epoch.
    pub nanoseconds: u64,
}

impl Timestamp {
    /// The epoch.
    pub const UNIX_EPOCH: Timestamp = Timestamp { nanoseconds: 0 };

    /// From nanoseconds since the Unix epoch.
    pub const fn from_nanos(nanoseconds: u64) -> Self {
        Timestamp { nanoseconds }
    }

    /// As nanoseconds since the Unix epoch.
    pub const fn as_nanos(self) -> u64 {
        self.nanoseconds
    }

    /// Read from the generated capnp reader.
    pub fn from_reader(reader: &types_capnp::timestamp::Reader<'_>) -> Self {
        Timestamp {
            nanoseconds: reader.get_nanoseconds(),
        }
    }

    /// Write into the generated capnp builder.
    pub fn write_to(&self, builder: &mut types_capnp::timestamp::Builder<'_>) {
        builder.set_nanoseconds(self.nanoseconds);
    }
}

impl From<&types_capnp::timestamp::Reader<'_>> for Timestamp {
    fn from(reader: &types_capnp::timestamp::Reader<'_>) -> Self {
        Timestamp::from_reader(reader)
    }
}

impl From<&types_capnp::timestamp::Builder<'_>> for Timestamp {
    fn from(builder: &types_capnp::timestamp::Builder<'_>) -> Self {
        Timestamp::from_reader(&builder.reborrow_as_reader())
    }
}

/// Monotonic time, immune to NTP/DST/manual adjustments — for timeouts,
/// benchmarks and interval measurement. Contract: `T.MonotonicTimestamp`.
#[derive(Copy, Clone, Debug, Default, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct MonotonicTimestamp {
    /// Nanoseconds on the monotonic clock.
    pub nanoseconds: u64,
}

impl MonotonicTimestamp {
    /// From nanoseconds on the monotonic clock.
    pub const fn from_nanos(nanoseconds: u64) -> Self {
        MonotonicTimestamp { nanoseconds }
    }

    /// As nanoseconds on the monotonic clock.
    pub const fn as_nanos(self) -> u64 {
        self.nanoseconds
    }

    /// Read from the generated capnp reader.
    pub fn from_reader(reader: &types_capnp::monotonic_timestamp::Reader<'_>) -> Self {
        MonotonicTimestamp {
            nanoseconds: reader.get_nanoseconds(),
        }
    }

    /// Write into the generated capnp builder.
    pub fn write_to(&self, builder: &mut types_capnp::monotonic_timestamp::Builder<'_>) {
        builder.set_nanoseconds(self.nanoseconds);
    }
}

impl From<&types_capnp::monotonic_timestamp::Reader<'_>> for MonotonicTimestamp {
    fn from(reader: &types_capnp::monotonic_timestamp::Reader<'_>) -> Self {
        MonotonicTimestamp::from_reader(reader)
    }
}

impl From<&types_capnp::monotonic_timestamp::Builder<'_>> for MonotonicTimestamp {
    fn from(builder: &types_capnp::monotonic_timestamp::Builder<'_>) -> Self {
        MonotonicTimestamp::from_reader(&builder.reborrow_as_reader())
    }
}

/// A protocol version. Contract: `T.Version`.
///
/// `epoch` is the packaging-epoch convention (0 = none), so `1:2.3.4` is
/// version 2.3.4 with epoch 1. Absent `preRelease`/`buildMetadata` are the
/// empty string. `T.Version.metadata` (backend bookkeeping) is deliberately
/// **not** modelled: the generated type remains the channel for a backend that
/// needs it, and core stays lossless on everything it claims.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Version {
    /// Major.
    pub major: u32,
    /// Minor.
    pub minor: u32,
    /// Patch.
    pub patch: u32,
    /// Pre-release identifier (empty when absent).
    pub pre_release: String,
    /// Build metadata (empty when absent).
    pub build_metadata: String,
    /// Packaging epoch (0 = none).
    pub epoch: u32,
}

impl Version {
    /// A bare `major.minor.patch`.
    pub fn new(major: u32, minor: u32, patch: u32) -> Self {
        Version {
            major,
            minor,
            patch,
            pre_release: String::new(),
            build_metadata: String::new(),
            epoch: 0,
        }
    }

    /// Read from the generated capnp reader.
    pub fn from_reader(reader: &types_capnp::version::Reader<'_>) -> Self {
        Version {
            major: reader.get_major(),
            minor: reader.get_minor(),
            patch: reader.get_patch(),
            pre_release: crate::core::error::text_or_empty(reader.get_pre_release()),
            build_metadata: crate::core::error::text_or_empty(reader.get_build_metadata()),
            epoch: reader.get_epoch(),
        }
    }

    /// Write into the generated capnp builder. Absent strings stay absent.
    pub fn write_to(&self, builder: &mut types_capnp::version::Builder<'_>) {
        builder.set_major(self.major);
        builder.set_minor(self.minor);
        builder.set_patch(self.patch);
        if !self.pre_release.is_empty() {
            builder.set_pre_release(self.pre_release.as_str());
        }
        if !self.build_metadata.is_empty() {
            builder.set_build_metadata(self.build_metadata.as_str());
        }
        builder.set_epoch(self.epoch);
    }
}

impl From<&types_capnp::version::Reader<'_>> for Version {
    fn from(reader: &types_capnp::version::Reader<'_>) -> Self {
        Version::from_reader(reader)
    }
}

impl From<&types_capnp::version::Builder<'_>> for Version {
    fn from(builder: &types_capnp::version::Builder<'_>) -> Self {
        Version::from_reader(&builder.reborrow_as_reader())
    }
}

impl core::fmt::Display for Version {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        if self.epoch != 0 {
            write!(f, "{}:", self.epoch)?;
        }
        write!(f, "{}.{}.{}", self.major, self.minor, self.patch)?;
        if !self.pre_release.is_empty() {
            write!(f, "-{}", self.pre_release)?;
        }
        if !self.build_metadata.is_empty() {
            write!(f, "+{}", self.build_metadata)?;
        }
        Ok(())
    }
}
