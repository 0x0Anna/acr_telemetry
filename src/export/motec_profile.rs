//! MoTeC LD channel profiles loaded from TOML (`motec_profiles/<name>.toml`).

use std::path::{Path, PathBuf};

use serde::Deserialize;

use crate::config;
use crate::record::{GraphicsRecord, PhysicsRecord};

/// Nominal wheel radius for rad/s → km/h (ACC wheel_angular_speed).
const WHEEL_RADIUS_M: f32 = 0.33;
const RAD_S_TO_KMH: f32 = WHEEL_RADIUS_M * 3.6;
const PSI_TO_BAR: f32 = 0.068_947_57;
const BRAKE_STATUS_THRESHOLD: f32 = 0.05;

#[derive(Debug, Clone, Deserialize)]
struct ProfileFile {
    #[serde(default)]
    description: String,
    channels: Vec<ChannelSpecFile>,
}

#[derive(Debug, Clone, Deserialize)]
struct ChannelSpecFile {
    name: String,
    #[serde(default)]
    unit: String,
    source: String,
    #[serde(default = "default_scale")]
    scale: f32,
    #[serde(default)]
    offset: f32,
    #[serde(default)]
    graphics: bool,
}

fn default_scale() -> f32 {
    1.0
}

#[derive(Debug, Clone)]
pub struct MotecProfile {
    pub id: String,
    pub description: String,
    pub channels: Vec<ProfileChannel>,
}

#[derive(Debug, Clone)]
pub struct ProfileChannel {
    pub name: String,
    pub unit: String,
    pub source: ChannelSource,
    pub scale: f32,
    pub offset: f32,
    pub graphics_only: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ChannelSource {
    Time,
    SpeedKmh,
    Rpm,
    Gas,
    Brake,
    SteerAngle,
    Gear,
    GForceX,
    GForceY,
    GForceTotal,
    EngineRotation,
    GearOk,
    BrakeStatus,
    SuspensionTravelFl,
    SuspensionTravelFr,
    SuspensionTravelRl,
    SuspensionTravelRr,
    SuspensionTravelMmFl,
    SuspensionTravelMmFr,
    SuspensionTravelMmRl,
    SuspensionTravelMmRr,
    CarPosX,
    CarPosY,
    CarPosZ,
    TyreContactXFl,
    TyreContactYFl,
    TyreContactZFl,
    TyreContactXFr,
    TyreContactYFr,
    TyreContactZFr,
    TyreContactXRl,
    TyreContactYRl,
    TyreContactZRl,
    TyreContactXRr,
    TyreContactYRr,
    TyreContactZRr,
    TyreTempCFl,
    TyreTempCFr,
    TyreTempCRl,
    TyreTempCRr,
    WheelPressureBarFl,
    WheelPressureBarFr,
    WheelPressureBarRl,
    WheelPressureBarRr,
    BrakeTempCFl,
    BrakeTempCFr,
    BrakeTempCRl,
    BrakeTempCRr,
    TyreWearPctFl,
    TyreWearPctFr,
    TyreWearPctRl,
    TyreWearPctRr,
    WheelSpeedKmhFl,
    WheelSpeedKmhFr,
    WheelSpeedKmhRl,
    WheelSpeedKmhRr,
    WheelSpeedKmhFront,
    WheelSpeedKmhRear,
    WheelSlipMax,
    GraphicsPosX,
    GraphicsPosY,
    GraphicsPosZ,
    Clutch,
    Fuel,
    Tc,
    Abs,
    TurboBoost,
    AirTemp,
    RoadTemp,
    WaterTemp,
    Heading,
    Pitch,
    Roll,
    BrakePressureFl,
    BrakePressureFr,
    BrakePressureRl,
    BrakePressureRr,
    SlipAngleFl,
    SlipAngleFr,
    SlipAngleRl,
    SlipAngleRr,
    RideHeightFront,
    RideHeightRear,
    CarDamageFront,
    CarDamageRear,
    CarDamageLeft,
    CarDamageRight,
    CarDamageCenter,
    WheelLoadFl,
    WheelLoadFr,
    WheelLoadRl,
    WheelLoadRr,
    WheelSlipFl,
    WheelSlipFr,
    WheelSlipRl,
    WheelSlipRr,
    CamberRadFl,
    CamberRadFr,
    CamberRadRl,
    CamberRadRr,
    SuspensionDamageFl,
    SuspensionDamageFr,
    SuspensionDamageRl,
    SuspensionDamageRr,
    SlipRatioFl,
    SlipRatioFr,
    SlipRatioRl,
    SlipRatioRr,
    PadLifeFl,
    PadLifeFr,
    PadLifeRl,
    PadLifeRr,
    DiscLifeFl,
    DiscLifeFr,
    DiscLifeRl,
    DiscLifeRr,
    FrontBrakeCompound,
    RearBrakeCompound,
    TyreDirtyLevelFl,
    TyreDirtyLevelFr,
    TyreDirtyLevelRl,
    TyreDirtyLevelRr,
    TyreTempIFl,
    TyreTempIFr,
    TyreTempIRl,
    TyreTempIRr,
    TyreTempMFl,
    TyreTempMFr,
    TyreTempMRl,
    TyreTempMRr,
    TyreTempOFl,
    TyreTempOFr,
    TyreTempORl,
    TyreTempORr,
    TyreTempExtraFl,
    TyreTempExtraFr,
    TyreTempExtraRl,
    TyreTempExtraRr,
    TyreContactNormalXFl,
    TyreContactNormalYFl,
    TyreContactNormalZFl,
    TyreContactNormalXFr,
    TyreContactNormalYFr,
    TyreContactNormalZFr,
    TyreContactNormalXRl,
    TyreContactNormalYRl,
    TyreContactNormalZRl,
    TyreContactNormalXRr,
    TyreContactNormalYRr,
    TyreContactNormalZRr,
    TyreContactHeadingXFl,
    TyreContactHeadingYFl,
    TyreContactHeadingZFl,
    TyreContactHeadingXFr,
    TyreContactHeadingYFr,
    TyreContactHeadingZFr,
    TyreContactHeadingXRl,
    TyreContactHeadingYRl,
    TyreContactHeadingZRl,
    TyreContactHeadingXRr,
    TyreContactHeadingYRr,
    TyreContactHeadingZRr,
    VelocityX,
    VelocityY,
    VelocityZ,
    LocalVelocityX,
    LocalVelocityY,
    LocalVelocityZ,
    LocalAngularVelX,
    LocalAngularVelY,
    LocalAngularVelZ,
    FinalFf,
    BrakeBias,
    TcInAction,
    AbsInAction,
    PitLimiterOn,
    IsAiControlled,
    Drs,
    CgHeight,
    NumberOfTyresOut,
    KersCharge,
    KersInput,
    KersCurrentKj,
    Ballast,
    AirDensity,
    PerformanceMeter,
    EngineBrake,
    ErsRecoveryLevel,
    ErsPowerLevel,
    ErsHeatCharging,
    ErsIsCharging,
    DrsAvailable,
    DrsEnabled,
    P2pActivation,
    P2pStatus,
    CurrentMaxRpm,
    MzFl,
    MzFr,
    MzRl,
    MzRr,
    FzFl,
    FzFr,
    FzRl,
    FzRr,
    MyFl,
    MyFr,
    MyRl,
    MyRr,
    KerbVibration,
    SlipVibration,
    GVibration,
    AbsVibration,
    AutoshifterOn,
    IgnitionOn,
    StarterEngineOn,
    IsEngineRunning,
    PacketId,
}

impl ChannelSource {
    fn parse(s: &str) -> Result<Self, String> {
        use ChannelSource::*;
        let v = match s {
            "time" => Time,
            "speed_kmh" => SpeedKmh,
            "rpm" => Rpm,
            "gas" => Gas,
            "brake" => Brake,
            "steer_angle" => SteerAngle,
            "gear" => Gear,
            "g_force_x" => GForceX,
            "g_force_y" => GForceY,
            "g_force_total" => GForceTotal,
            "engine_rotation" => EngineRotation,
            "gear_ok" => GearOk,
            "brake_status" => BrakeStatus,
            "suspension_travel_fl" => SuspensionTravelFl,
            "suspension_travel_fr" => SuspensionTravelFr,
            "suspension_travel_rl" => SuspensionTravelRl,
            "suspension_travel_rr" => SuspensionTravelRr,
            "suspension_travel_mm_fl" => SuspensionTravelMmFl,
            "suspension_travel_mm_fr" => SuspensionTravelMmFr,
            "suspension_travel_mm_rl" => SuspensionTravelMmRl,
            "suspension_travel_mm_rr" => SuspensionTravelMmRr,
            "car_pos_x" => CarPosX,
            "car_pos_y" => CarPosY,
            "car_pos_z" => CarPosZ,
            "tyre_contact_x_fl" => TyreContactXFl,
            "tyre_contact_y_fl" => TyreContactYFl,
            "tyre_contact_z_fl" => TyreContactZFl,
            "tyre_contact_x_fr" => TyreContactXFr,
            "tyre_contact_y_fr" => TyreContactYFr,
            "tyre_contact_z_fr" => TyreContactZFr,
            "tyre_contact_x_rl" => TyreContactXRl,
            "tyre_contact_y_rl" => TyreContactYRl,
            "tyre_contact_z_rl" => TyreContactZRl,
            "tyre_contact_x_rr" => TyreContactXRr,
            "tyre_contact_y_rr" => TyreContactYRr,
            "tyre_contact_z_rr" => TyreContactZRr,
            "tyre_temp_c_fl" => TyreTempCFl,
            "tyre_temp_c_fr" => TyreTempCFr,
            "tyre_temp_c_rl" => TyreTempCRl,
            "tyre_temp_c_rr" => TyreTempCRr,
            "wheel_pressure_bar_fl" => WheelPressureBarFl,
            "wheel_pressure_bar_fr" => WheelPressureBarFr,
            "wheel_pressure_bar_rl" => WheelPressureBarRl,
            "wheel_pressure_bar_rr" => WheelPressureBarRr,
            "brake_temp_c_fl" => BrakeTempCFl,
            "brake_temp_c_fr" => BrakeTempCFr,
            "brake_temp_c_rl" => BrakeTempCRl,
            "brake_temp_c_rr" => BrakeTempCRr,
            "tyre_wear_pct_fl" => TyreWearPctFl,
            "tyre_wear_pct_fr" => TyreWearPctFr,
            "tyre_wear_pct_rl" => TyreWearPctRl,
            "tyre_wear_pct_rr" => TyreWearPctRr,
            "wheel_speed_kmh_fl" => WheelSpeedKmhFl,
            "wheel_speed_kmh_fr" => WheelSpeedKmhFr,
            "wheel_speed_kmh_rl" => WheelSpeedKmhRl,
            "wheel_speed_kmh_rr" => WheelSpeedKmhRr,
            "wheel_speed_kmh_front" => WheelSpeedKmhFront,
            "wheel_speed_kmh_rear" => WheelSpeedKmhRear,
            "wheel_slip_max" => WheelSlipMax,
            "graphics_pos_x" => GraphicsPosX,
            "graphics_pos_y" => GraphicsPosY,
            "graphics_pos_z" => GraphicsPosZ,
            "clutch" => Clutch,
            "fuel" => Fuel,
            "tc" => Tc,
            "abs" => Abs,
            "turbo_boost" => TurboBoost,
            "air_temp" => AirTemp,
            "road_temp" => RoadTemp,
            "water_temp" => WaterTemp,
            "heading" => Heading,
            "pitch" => Pitch,
            "roll" => Roll,
            "brake_pressure_fl" => BrakePressureFl,
            "brake_pressure_fr" => BrakePressureFr,
            "brake_pressure_rl" => BrakePressureRl,
            "brake_pressure_rr" => BrakePressureRr,
            "slip_angle_fl" => SlipAngleFl,
            "slip_angle_fr" => SlipAngleFr,
            "slip_angle_rl" => SlipAngleRl,
            "slip_angle_rr" => SlipAngleRr,
            "ride_height_front" => RideHeightFront,
            "ride_height_rear" => RideHeightRear,
            "car_damage_front" => CarDamageFront,
            "car_damage_rear" => CarDamageRear,
            "car_damage_left" => CarDamageLeft,
            "car_damage_right" => CarDamageRight,
            "car_damage_center" => CarDamageCenter,
            "wheel_load_fl" => WheelLoadFl,
            "wheel_load_fr" => WheelLoadFr,
            "wheel_load_rl" => WheelLoadRl,
            "wheel_load_rr" => WheelLoadRr,
            "wheel_slip_fl" => WheelSlipFl,
            "wheel_slip_fr" => WheelSlipFr,
            "wheel_slip_rl" => WheelSlipRl,
            "wheel_slip_rr" => WheelSlipRr,
            "camber_rad_fl" => CamberRadFl,
            "camber_rad_fr" => CamberRadFr,
            "camber_rad_rl" => CamberRadRl,
            "camber_rad_rr" => CamberRadRr,
            "suspension_damage_fl" => SuspensionDamageFl,
            "suspension_damage_fr" => SuspensionDamageFr,
            "suspension_damage_rl" => SuspensionDamageRl,
            "suspension_damage_rr" => SuspensionDamageRr,
            "slip_ratio_fl" => SlipRatioFl,
            "slip_ratio_fr" => SlipRatioFr,
            "slip_ratio_rl" => SlipRatioRl,
            "slip_ratio_rr" => SlipRatioRr,
            "pad_life_fl" => PadLifeFl,
            "pad_life_fr" => PadLifeFr,
            "pad_life_rl" => PadLifeRl,
            "pad_life_rr" => PadLifeRr,
            "disc_life_fl" => DiscLifeFl,
            "disc_life_fr" => DiscLifeFr,
            "disc_life_rl" => DiscLifeRl,
            "disc_life_rr" => DiscLifeRr,
            "front_brake_compound" => FrontBrakeCompound,
            "rear_brake_compound" => RearBrakeCompound,
            "tyre_dirty_level_fl" => TyreDirtyLevelFl,
            "tyre_dirty_level_fr" => TyreDirtyLevelFr,
            "tyre_dirty_level_rl" => TyreDirtyLevelRl,
            "tyre_dirty_level_rr" => TyreDirtyLevelRr,
            "tyre_temp_i_fl" => TyreTempIFl,
            "tyre_temp_i_fr" => TyreTempIFr,
            "tyre_temp_i_rl" => TyreTempIRl,
            "tyre_temp_i_rr" => TyreTempIRr,
            "tyre_temp_m_fl" => TyreTempMFl,
            "tyre_temp_m_fr" => TyreTempMFr,
            "tyre_temp_m_rl" => TyreTempMRl,
            "tyre_temp_m_rr" => TyreTempMRr,
            "tyre_temp_o_fl" => TyreTempOFl,
            "tyre_temp_o_fr" => TyreTempOFr,
            "tyre_temp_o_rl" => TyreTempORl,
            "tyre_temp_o_rr" => TyreTempORr,
            "tyre_temp_extra_fl" => TyreTempExtraFl,
            "tyre_temp_extra_fr" => TyreTempExtraFr,
            "tyre_temp_extra_rl" => TyreTempExtraRl,
            "tyre_temp_extra_rr" => TyreTempExtraRr,
            "tyre_contact_normal_x_fl" => TyreContactNormalXFl,
            "tyre_contact_normal_y_fl" => TyreContactNormalYFl,
            "tyre_contact_normal_z_fl" => TyreContactNormalZFl,
            "tyre_contact_normal_x_fr" => TyreContactNormalXFr,
            "tyre_contact_normal_y_fr" => TyreContactNormalYFr,
            "tyre_contact_normal_z_fr" => TyreContactNormalZFr,
            "tyre_contact_normal_x_rl" => TyreContactNormalXRl,
            "tyre_contact_normal_y_rl" => TyreContactNormalYRl,
            "tyre_contact_normal_z_rl" => TyreContactNormalZRl,
            "tyre_contact_normal_x_rr" => TyreContactNormalXRr,
            "tyre_contact_normal_y_rr" => TyreContactNormalYRr,
            "tyre_contact_normal_z_rr" => TyreContactNormalZRr,
            "tyre_contact_heading_x_fl" => TyreContactHeadingXFl,
            "tyre_contact_heading_y_fl" => TyreContactHeadingYFl,
            "tyre_contact_heading_z_fl" => TyreContactHeadingZFl,
            "tyre_contact_heading_x_fr" => TyreContactHeadingXFr,
            "tyre_contact_heading_y_fr" => TyreContactHeadingYFr,
            "tyre_contact_heading_z_fr" => TyreContactHeadingZFr,
            "tyre_contact_heading_x_rl" => TyreContactHeadingXRl,
            "tyre_contact_heading_y_rl" => TyreContactHeadingYRl,
            "tyre_contact_heading_z_rl" => TyreContactHeadingZRl,
            "tyre_contact_heading_x_rr" => TyreContactHeadingXRr,
            "tyre_contact_heading_y_rr" => TyreContactHeadingYRr,
            "tyre_contact_heading_z_rr" => TyreContactHeadingZRr,
            "velocity_x" => VelocityX,
            "velocity_y" => VelocityY,
            "velocity_z" => VelocityZ,
            "local_velocity_x" => LocalVelocityX,
            "local_velocity_y" => LocalVelocityY,
            "local_velocity_z" => LocalVelocityZ,
            "local_angular_vel_x" => LocalAngularVelX,
            "local_angular_vel_y" => LocalAngularVelY,
            "local_angular_vel_z" => LocalAngularVelZ,
            "final_ff" => FinalFf,
            "brake_bias" => BrakeBias,
            "tc_in_action" => TcInAction,
            "abs_in_action" => AbsInAction,
            "pit_limiter_on" => PitLimiterOn,
            "is_ai_controlled" => IsAiControlled,
            "drs" => Drs,
            "cg_height" => CgHeight,
            "number_of_tyres_out" => NumberOfTyresOut,
            "kers_charge" => KersCharge,
            "kers_input" => KersInput,
            "kers_current_kj" => KersCurrentKj,
            "ballast" => Ballast,
            "air_density" => AirDensity,
            "performance_meter" => PerformanceMeter,
            "engine_brake" => EngineBrake,
            "ers_recovery_level" => ErsRecoveryLevel,
            "ers_power_level" => ErsPowerLevel,
            "ers_heat_charging" => ErsHeatCharging,
            "ers_is_charging" => ErsIsCharging,
            "drs_available" => DrsAvailable,
            "drs_enabled" => DrsEnabled,
            "p2p_activation" => P2pActivation,
            "p2p_status" => P2pStatus,
            "current_max_rpm" => CurrentMaxRpm,
            "mz_fl" => MzFl,
            "mz_fr" => MzFr,
            "mz_rl" => MzRl,
            "mz_rr" => MzRr,
            "fz_fl" => FzFl,
            "fz_fr" => FzFr,
            "fz_rl" => FzRl,
            "fz_rr" => FzRr,
            "my_fl" => MyFl,
            "my_fr" => MyFr,
            "my_rl" => MyRl,
            "my_rr" => MyRr,
            "kerb_vibration" => KerbVibration,
            "slip_vibration" => SlipVibration,
            "g_vibration" => GVibration,
            "abs_vibration" => AbsVibration,
            "autoshifter_on" => AutoshifterOn,
            "ignition_on" => IgnitionOn,
            "starter_engine_on" => StarterEngineOn,
            "is_engine_running" => IsEngineRunning,
            "packet_id" => PacketId,
            other => return Err(format!("unknown MoTeC channel source '{other}'")),
        };
        Ok(v)
    }
}

/// Load profile by id (filename without `.toml`).
pub fn load_profile(profile_id: &str, profiles_dir: Option<&Path>) -> Result<MotecProfile, String> {
    let id = profile_id.trim();
    if id.is_empty() {
        return Err("MoTeC profile name is empty".into());
    }
    if let Some(path) = find_profile_file(id, profiles_dir) {
        let text = std::fs::read_to_string(&path)
            .map_err(|e| format!("read {}: {e}", path.display()))?;
        return parse_profile_text(id, &text);
    }
    let text = builtin_profile_toml(id)?;
    parse_profile_text(id, text)
}

fn find_profile_file(profile_id: &str, profiles_dir: Option<&Path>) -> Option<PathBuf> {
    let filename = format!("{profile_id}.toml");
    let mut candidates: Vec<PathBuf> = Vec::new();
    if let Some(dir) = profiles_dir {
        candidates.push(dir.join(&filename));
    }
    if let Some(base) = config::base_dir() {
        candidates.push(base.join("motec_profiles").join(&filename));
    }
    if let Ok(cwd) = std::env::current_dir() {
        candidates.push(cwd.join("motec_profiles").join(&filename));
    }
    candidates.into_iter().find(|p| p.is_file())
}

fn builtin_profile_toml(profile_id: &str) -> Result<&'static str, String> {
    match profile_id {
        "rbr" => Ok(include_str!("../../config/motec_profiles/rbr.toml")),
        "rally" => Ok(include_str!("../../config/motec_profiles/rally.toml")),
        "all_data" => Ok(include_str!("../../config/motec_profiles/all_data.toml")),
        other => Err(format!(
            "MoTeC profile '{other}' not found in motec_profiles/ and no built-in profile"
        )),
    }
}

fn parse_profile_text(id: &str, text: &str) -> Result<MotecProfile, String> {
    let file: ProfileFile = toml::from_str(text).map_err(|e| format!("profile '{id}' TOML: {e}"))?;
    if file.channels.is_empty() {
        return Err(format!("profile '{id}' has no [[channels]]"));
    }
    let mut channels = Vec::with_capacity(file.channels.len());
    for ch in file.channels {
        let source = ChannelSource::parse(&ch.source)?;
        channels.push(ProfileChannel {
            name: ch.name,
            unit: ch.unit,
            source,
            scale: ch.scale,
            offset: ch.offset,
            graphics_only: ch.graphics,
        });
    }
    Ok(MotecProfile {
        id: id.to_string(),
        description: file.description,
        channels,
    })
}

/// Resolve profile using export config (see `MotecExportConfig`).
pub fn load_profile_from_config(
    profile_id: &str,
    profiles_dir: Option<&str>,
) -> Result<MotecProfile, String> {
    let dir = profiles_dir
        .filter(|s| !s.trim().is_empty())
        .map(config::resolve_path);
    load_profile(profile_id, dir.as_deref())
}

pub fn build_ld_channels(
    profile: &MotecProfile,
    records: &[PhysicsRecord],
    sample_rate_hz: u32,
    graphics: Option<(&[GraphicsRecord], u32)>,
) -> Result<Vec<(String, String, Vec<f32>)>, String> {
    let mut records = records.to_vec();
    crate::record::ensure_capture_times(&mut records, sample_rate_hz);
    let has_graphics = graphics.map(|(g, _)| !g.is_empty()).unwrap_or(false);
    let mut out = Vec::new();
    for ch in &profile.channels {
        if ch.graphics_only && !has_graphics {
            continue;
        }
        let mut data = extract_channel(
            ch.source,
            &records,
            sample_rate_hz,
            graphics,
        )?;
        if ch.scale != 1.0 || ch.offset != 0.0 {
            for v in &mut data {
                *v = *v * ch.scale + ch.offset;
            }
        }
        out.push((ch.name.clone(), ch.unit.clone(), data));
    }
    Ok(out)
}

fn extract_channel(
    source: ChannelSource,
    records: &[PhysicsRecord],
    sample_rate_hz: u32,
    graphics: Option<(&[GraphicsRecord], u32)>,
) -> Result<Vec<f32>, String> {
    let n = records.len();
    let _hz = sample_rate_hz.max(1) as f32;
    Ok(match source {
        ChannelSource::Time => PhysicsRecord::motec_time_secs(records),
        ChannelSource::SpeedKmh => records.iter().map(|r| r.speed_kmh).collect(),
        ChannelSource::Rpm => records.iter().map(|r| r.rpm as f32).collect(),
        ChannelSource::Gas => records.iter().map(|r| r.gas).collect(),
        ChannelSource::Brake => records.iter().map(|r| r.brake).collect(),
        ChannelSource::SteerAngle => records.iter().map(|r| r.steer_angle).collect(),
        ChannelSource::Gear => records.iter().map(|r| r.gear as f32).collect(),
        ChannelSource::GForceX => records.iter().map(|r| r.g_force.x).collect(),
        ChannelSource::GForceY => records.iter().map(|r| r.g_force.y).collect(),
        ChannelSource::GForceTotal => records
            .iter()
            .map(|r| (r.g_force.x * r.g_force.x + r.g_force.y * r.g_force.y).sqrt())
            .collect(),
        ChannelSource::EngineRotation => records
            .iter()
            .map(|r| r.rpm as f32 * std::f32::consts::TAU / 60.0)
            .collect(),
        ChannelSource::GearOk => records.iter().map(|r| (r.gear - 1) as f32).collect(),
        ChannelSource::BrakeStatus => records
            .iter()
            .map(|r| if r.brake > BRAKE_STATUS_THRESHOLD { 1.0 } else { 0.0 })
            .collect(),
        ChannelSource::SuspensionTravelFl => records
            .iter()
            .map(|r| r.suspension_travel.front_left)
            .collect(),
        ChannelSource::SuspensionTravelFr => records
            .iter()
            .map(|r| r.suspension_travel.front_right)
            .collect(),
        ChannelSource::SuspensionTravelRl => records
            .iter()
            .map(|r| r.suspension_travel.rear_left)
            .collect(),
        ChannelSource::SuspensionTravelRr => records
            .iter()
            .map(|r| r.suspension_travel.rear_right)
            .collect(),
        ChannelSource::SuspensionTravelMmFl => records
            .iter()
            .map(|r| r.suspension_travel.front_left * 1000.0)
            .collect(),
        ChannelSource::SuspensionTravelMmFr => records
            .iter()
            .map(|r| r.suspension_travel.front_right * 1000.0)
            .collect(),
        ChannelSource::SuspensionTravelMmRl => records
            .iter()
            .map(|r| r.suspension_travel.rear_left * 1000.0)
            .collect(),
        ChannelSource::SuspensionTravelMmRr => records
            .iter()
            .map(|r| r.suspension_travel.rear_right * 1000.0)
            .collect(),
        ChannelSource::CarPosX | ChannelSource::CarPosY | ChannelSource::CarPosZ => {
            let idx = match source {
                ChannelSource::CarPosX => 0,
                ChannelSource::CarPosY => 1,
                _ => 2,
            };
            records
                .iter()
                .map(|r| {
                    let p = &r.tyre_contact_point;
                    let vals = [
                        (p.front_left.x + p.front_right.x + p.rear_left.x + p.rear_right.x)
                            * 0.25,
                        (p.front_left.y + p.front_right.y + p.rear_left.y + p.rear_right.y)
                            * 0.25,
                        (p.front_left.z + p.front_right.z + p.rear_left.z + p.rear_right.z)
                            * 0.25,
                    ];
                    vals[idx]
                })
                .collect()
        }
        ChannelSource::TyreContactXFl => records
            .iter()
            .map(|r| r.tyre_contact_point.front_left.x)
            .collect(),
        ChannelSource::TyreContactYFl => records
            .iter()
            .map(|r| r.tyre_contact_point.front_left.y)
            .collect(),
        ChannelSource::TyreContactZFl => records
            .iter()
            .map(|r| r.tyre_contact_point.front_left.z)
            .collect(),
        ChannelSource::TyreContactXFr => records
            .iter()
            .map(|r| r.tyre_contact_point.front_right.x)
            .collect(),
        ChannelSource::TyreContactYFr => records
            .iter()
            .map(|r| r.tyre_contact_point.front_right.y)
            .collect(),
        ChannelSource::TyreContactZFr => records
            .iter()
            .map(|r| r.tyre_contact_point.front_right.z)
            .collect(),
        ChannelSource::TyreContactXRl => records
            .iter()
            .map(|r| r.tyre_contact_point.rear_left.x)
            .collect(),
        ChannelSource::TyreContactYRl => records
            .iter()
            .map(|r| r.tyre_contact_point.rear_left.y)
            .collect(),
        ChannelSource::TyreContactZRl => records
            .iter()
            .map(|r| r.tyre_contact_point.rear_left.z)
            .collect(),
        ChannelSource::TyreContactXRr => records
            .iter()
            .map(|r| r.tyre_contact_point.rear_right.x)
            .collect(),
        ChannelSource::TyreContactYRr => records
            .iter()
            .map(|r| r.tyre_contact_point.rear_right.y)
            .collect(),
        ChannelSource::TyreContactZRr => records
            .iter()
            .map(|r| r.tyre_contact_point.rear_right.z)
            .collect(),
        ChannelSource::TyreTempCFl => records
            .iter()
            .map(|r| r.tyre_core_temp.front_left - 273.15)
            .collect(),
        ChannelSource::TyreTempCFr => records
            .iter()
            .map(|r| r.tyre_core_temp.front_right - 273.15)
            .collect(),
        ChannelSource::TyreTempCRl => records
            .iter()
            .map(|r| r.tyre_core_temp.rear_left - 273.15)
            .collect(),
        ChannelSource::TyreTempCRr => records
            .iter()
            .map(|r| r.tyre_core_temp.rear_right - 273.15)
            .collect(),
        ChannelSource::WheelPressureBarFl => records
            .iter()
            .map(|r| r.wheel_pressure.front_left * PSI_TO_BAR)
            .collect(),
        ChannelSource::WheelPressureBarFr => records
            .iter()
            .map(|r| r.wheel_pressure.front_right * PSI_TO_BAR)
            .collect(),
        ChannelSource::WheelPressureBarRl => records
            .iter()
            .map(|r| r.wheel_pressure.rear_left * PSI_TO_BAR)
            .collect(),
        ChannelSource::WheelPressureBarRr => records
            .iter()
            .map(|r| r.wheel_pressure.rear_right * PSI_TO_BAR)
            .collect(),
        ChannelSource::BrakeTempCFl => records
            .iter()
            .map(|r| r.brake_temp.front_left - 273.15)
            .collect(),
        ChannelSource::BrakeTempCFr => records
            .iter()
            .map(|r| r.brake_temp.front_right - 273.15)
            .collect(),
        ChannelSource::BrakeTempCRl => records
            .iter()
            .map(|r| r.brake_temp.rear_left - 273.15)
            .collect(),
        ChannelSource::BrakeTempCRr => records
            .iter()
            .map(|r| r.brake_temp.rear_right - 273.15)
            .collect(),
        ChannelSource::TyreWearPctFl => records
            .iter()
            .map(|r| r.tyre_wear.front_left * 100.0)
            .collect(),
        ChannelSource::TyreWearPctFr => records
            .iter()
            .map(|r| r.tyre_wear.front_right * 100.0)
            .collect(),
        ChannelSource::TyreWearPctRl => records
            .iter()
            .map(|r| r.tyre_wear.rear_left * 100.0)
            .collect(),
        ChannelSource::TyreWearPctRr => records
            .iter()
            .map(|r| r.tyre_wear.rear_right * 100.0)
            .collect(),
        ChannelSource::WheelSpeedKmhFl => records
            .iter()
            .map(|r| r.wheel_angular_speed.front_left.abs() * RAD_S_TO_KMH)
            .collect(),
        ChannelSource::WheelSpeedKmhFr => records
            .iter()
            .map(|r| r.wheel_angular_speed.front_right.abs() * RAD_S_TO_KMH)
            .collect(),
        ChannelSource::WheelSpeedKmhRl => records
            .iter()
            .map(|r| r.wheel_angular_speed.rear_left.abs() * RAD_S_TO_KMH)
            .collect(),
        ChannelSource::WheelSpeedKmhRr => records
            .iter()
            .map(|r| r.wheel_angular_speed.rear_right.abs() * RAD_S_TO_KMH)
            .collect(),
        ChannelSource::WheelSpeedKmhFront => records
            .iter()
            .map(|r| {
                (r.wheel_angular_speed.front_left.abs() + r.wheel_angular_speed.front_right.abs())
                    * 0.5
                    * RAD_S_TO_KMH
            })
            .collect(),
        ChannelSource::WheelSpeedKmhRear => records
            .iter()
            .map(|r| {
                (r.wheel_angular_speed.rear_left.abs() + r.wheel_angular_speed.rear_right.abs())
                    * 0.5
                    * RAD_S_TO_KMH
            })
            .collect(),
        ChannelSource::WheelSlipMax => records
            .iter()
            .map(|r| {
                r.wheel_slip
                    .front_left
                    .max(r.wheel_slip.front_right)
                    .max(r.wheel_slip.rear_left)
                    .max(r.wheel_slip.rear_right)
            })
            .collect(),
        ChannelSource::GraphicsPosX => {
            let (g, _) = graphics.ok_or("graphics_pos_x requires graphics sidecar")?;
            resample_graphics_to_len(g, n, |rec| rec.car_coordinates_x)
        }
        ChannelSource::GraphicsPosY => {
            let (g, _) = graphics.ok_or("graphics_pos_y requires graphics sidecar")?;
            resample_graphics_to_len(g, n, |rec| rec.car_coordinates_y)
        }
        ChannelSource::GraphicsPosZ => {
            let (g, _) = graphics.ok_or("graphics_pos_z requires graphics sidecar")?;
            resample_graphics_to_len(g, n, |rec| rec.car_coordinates_z)
        }
        ChannelSource::Clutch => records.iter().map(|r| r.clutch).collect(),
        ChannelSource::Fuel => records.iter().map(|r| r.fuel).collect(),
        ChannelSource::Tc => records.iter().map(|r| r.tc).collect(),
        ChannelSource::Abs => records.iter().map(|r| r.abs).collect(),
        ChannelSource::TurboBoost => records.iter().map(|r| r.turbo_boost).collect(),
        ChannelSource::AirTemp => records.iter().map(|r| r.air_temp).collect(),
        ChannelSource::RoadTemp => records.iter().map(|r| r.road_temp).collect(),
        ChannelSource::WaterTemp => records.iter().map(|r| r.water_temp).collect(),
        ChannelSource::Heading => records.iter().map(|r| r.heading).collect(),
        ChannelSource::Pitch => records.iter().map(|r| r.pitch).collect(),
        ChannelSource::Roll => records.iter().map(|r| r.roll).collect(),
        ChannelSource::BrakePressureFl => records
            .iter()
            .map(|r| r.brake_pressure.front_left)
            .collect(),
        ChannelSource::BrakePressureFr => records
            .iter()
            .map(|r| r.brake_pressure.front_right)
            .collect(),
        ChannelSource::BrakePressureRl => records
            .iter()
            .map(|r| r.brake_pressure.rear_left)
            .collect(),
        ChannelSource::BrakePressureRr => records
            .iter()
            .map(|r| r.brake_pressure.rear_right)
            .collect(),
        ChannelSource::SlipAngleFl => records.iter().map(|r| r.slip_angle.front_left).collect(),
        ChannelSource::SlipAngleFr => records.iter().map(|r| r.slip_angle.front_right).collect(),
        ChannelSource::SlipAngleRl => records.iter().map(|r| r.slip_angle.rear_left).collect(),
        ChannelSource::SlipAngleRr => records.iter().map(|r| r.slip_angle.rear_right).collect(),
        ChannelSource::RideHeightFront => records.iter().map(|r| r.ride_height_front).collect(),
        ChannelSource::RideHeightRear => records.iter().map(|r| r.ride_height_rear).collect(),
        ChannelSource::CarDamageFront => records.iter().map(|r| r.car_damage.front).collect(),
        ChannelSource::CarDamageRear => records.iter().map(|r| r.car_damage.rear).collect(),
        ChannelSource::CarDamageLeft => records.iter().map(|r| r.car_damage.left).collect(),
        ChannelSource::CarDamageRight => records.iter().map(|r| r.car_damage.right).collect(),
        ChannelSource::CarDamageCenter => records.iter().map(|r| r.car_damage.center).collect(),
        ChannelSource::WheelLoadFl => records.iter().map(|r| r.wheel_load.front_left).collect(),
        ChannelSource::WheelLoadFr => records.iter().map(|r| r.wheel_load.front_right).collect(),
        ChannelSource::WheelLoadRl => records.iter().map(|r| r.wheel_load.rear_left).collect(),
        ChannelSource::WheelLoadRr => records.iter().map(|r| r.wheel_load.rear_right).collect(),
        ChannelSource::WheelSlipFl => records.iter().map(|r| r.wheel_slip.front_left).collect(),
        ChannelSource::WheelSlipFr => records.iter().map(|r| r.wheel_slip.front_right).collect(),
        ChannelSource::WheelSlipRl => records.iter().map(|r| r.wheel_slip.rear_left).collect(),
        ChannelSource::WheelSlipRr => records.iter().map(|r| r.wheel_slip.rear_right).collect(),
        ChannelSource::CamberRadFl => records.iter().map(|r| r.camber_rad.front_left).collect(),
        ChannelSource::CamberRadFr => records.iter().map(|r| r.camber_rad.front_right).collect(),
        ChannelSource::CamberRadRl => records.iter().map(|r| r.camber_rad.rear_left).collect(),
        ChannelSource::CamberRadRr => records.iter().map(|r| r.camber_rad.rear_right).collect(),
        ChannelSource::SuspensionDamageFl => records
            .iter()
            .map(|r| r.suspension_damage.front_left)
            .collect(),
        ChannelSource::SuspensionDamageFr => records
            .iter()
            .map(|r| r.suspension_damage.front_right)
            .collect(),
        ChannelSource::SuspensionDamageRl => records
            .iter()
            .map(|r| r.suspension_damage.rear_left)
            .collect(),
        ChannelSource::SuspensionDamageRr => records
            .iter()
            .map(|r| r.suspension_damage.rear_right)
            .collect(),
        ChannelSource::SlipRatioFl => records.iter().map(|r| r.slip_ratio.front_left).collect(),
        ChannelSource::SlipRatioFr => records.iter().map(|r| r.slip_ratio.front_right).collect(),
        ChannelSource::SlipRatioRl => records.iter().map(|r| r.slip_ratio.rear_left).collect(),
        ChannelSource::SlipRatioRr => records.iter().map(|r| r.slip_ratio.rear_right).collect(),
        ChannelSource::PadLifeFl => records.iter().map(|r| r.pad_life.front_left).collect(),
        ChannelSource::PadLifeFr => records.iter().map(|r| r.pad_life.front_right).collect(),
        ChannelSource::PadLifeRl => records.iter().map(|r| r.pad_life.rear_left).collect(),
        ChannelSource::PadLifeRr => records.iter().map(|r| r.pad_life.rear_right).collect(),
        ChannelSource::DiscLifeFl => records.iter().map(|r| r.disc_life.front_left).collect(),
        ChannelSource::DiscLifeFr => records.iter().map(|r| r.disc_life.front_right).collect(),
        ChannelSource::DiscLifeRl => records.iter().map(|r| r.disc_life.rear_left).collect(),
        ChannelSource::DiscLifeRr => records.iter().map(|r| r.disc_life.rear_right).collect(),
        ChannelSource::FrontBrakeCompound => records
            .iter()
            .map(|r| r.front_brake_compound as f32)
            .collect(),
        ChannelSource::RearBrakeCompound => records
            .iter()
            .map(|r| r.rear_brake_compound as f32)
            .collect(),
        ChannelSource::TyreDirtyLevelFl => records
            .iter()
            .map(|r| r.tyre_dirty_level.front_left)
            .collect(),
        ChannelSource::TyreDirtyLevelFr => records
            .iter()
            .map(|r| r.tyre_dirty_level.front_right)
            .collect(),
        ChannelSource::TyreDirtyLevelRl => records
            .iter()
            .map(|r| r.tyre_dirty_level.rear_left)
            .collect(),
        ChannelSource::TyreDirtyLevelRr => records
            .iter()
            .map(|r| r.tyre_dirty_level.rear_right)
            .collect(),
        ChannelSource::TyreTempIFl => records.iter().map(|r| r.tyre_temp_i.front_left).collect(),
        ChannelSource::TyreTempIFr => records.iter().map(|r| r.tyre_temp_i.front_right).collect(),
        ChannelSource::TyreTempIRl => records.iter().map(|r| r.tyre_temp_i.rear_left).collect(),
        ChannelSource::TyreTempIRr => records.iter().map(|r| r.tyre_temp_i.rear_right).collect(),
        ChannelSource::TyreTempMFl => records.iter().map(|r| r.tyre_temp_m.front_left).collect(),
        ChannelSource::TyreTempMFr => records.iter().map(|r| r.tyre_temp_m.front_right).collect(),
        ChannelSource::TyreTempMRl => records.iter().map(|r| r.tyre_temp_m.rear_left).collect(),
        ChannelSource::TyreTempMRr => records.iter().map(|r| r.tyre_temp_m.rear_right).collect(),
        ChannelSource::TyreTempOFl => records.iter().map(|r| r.tyre_temp_o.front_left).collect(),
        ChannelSource::TyreTempOFr => records.iter().map(|r| r.tyre_temp_o.front_right).collect(),
        ChannelSource::TyreTempORl => records.iter().map(|r| r.tyre_temp_o.rear_left).collect(),
        ChannelSource::TyreTempORr => records.iter().map(|r| r.tyre_temp_o.rear_right).collect(),
        ChannelSource::TyreTempExtraFl => records
            .iter()
            .map(|r| r.tyre_temp_extra.front_left)
            .collect(),
        ChannelSource::TyreTempExtraFr => records
            .iter()
            .map(|r| r.tyre_temp_extra.front_right)
            .collect(),
        ChannelSource::TyreTempExtraRl => records
            .iter()
            .map(|r| r.tyre_temp_extra.rear_left)
            .collect(),
        ChannelSource::TyreTempExtraRr => records
            .iter()
            .map(|r| r.tyre_temp_extra.rear_right)
            .collect(),
        ChannelSource::TyreContactNormalXFl => records
            .iter()
            .map(|r| r.tyre_contact_normal.front_left.x)
            .collect(),
        ChannelSource::TyreContactNormalYFl => records
            .iter()
            .map(|r| r.tyre_contact_normal.front_left.y)
            .collect(),
        ChannelSource::TyreContactNormalZFl => records
            .iter()
            .map(|r| r.tyre_contact_normal.front_left.z)
            .collect(),
        ChannelSource::TyreContactNormalXFr => records
            .iter()
            .map(|r| r.tyre_contact_normal.front_right.x)
            .collect(),
        ChannelSource::TyreContactNormalYFr => records
            .iter()
            .map(|r| r.tyre_contact_normal.front_right.y)
            .collect(),
        ChannelSource::TyreContactNormalZFr => records
            .iter()
            .map(|r| r.tyre_contact_normal.front_right.z)
            .collect(),
        ChannelSource::TyreContactNormalXRl => records
            .iter()
            .map(|r| r.tyre_contact_normal.rear_left.x)
            .collect(),
        ChannelSource::TyreContactNormalYRl => records
            .iter()
            .map(|r| r.tyre_contact_normal.rear_left.y)
            .collect(),
        ChannelSource::TyreContactNormalZRl => records
            .iter()
            .map(|r| r.tyre_contact_normal.rear_left.z)
            .collect(),
        ChannelSource::TyreContactNormalXRr => records
            .iter()
            .map(|r| r.tyre_contact_normal.rear_right.x)
            .collect(),
        ChannelSource::TyreContactNormalYRr => records
            .iter()
            .map(|r| r.tyre_contact_normal.rear_right.y)
            .collect(),
        ChannelSource::TyreContactNormalZRr => records
            .iter()
            .map(|r| r.tyre_contact_normal.rear_right.z)
            .collect(),
        ChannelSource::TyreContactHeadingXFl => records
            .iter()
            .map(|r| r.tyre_contact_heading.front_left.x)
            .collect(),
        ChannelSource::TyreContactHeadingYFl => records
            .iter()
            .map(|r| r.tyre_contact_heading.front_left.y)
            .collect(),
        ChannelSource::TyreContactHeadingZFl => records
            .iter()
            .map(|r| r.tyre_contact_heading.front_left.z)
            .collect(),
        ChannelSource::TyreContactHeadingXFr => records
            .iter()
            .map(|r| r.tyre_contact_heading.front_right.x)
            .collect(),
        ChannelSource::TyreContactHeadingYFr => records
            .iter()
            .map(|r| r.tyre_contact_heading.front_right.y)
            .collect(),
        ChannelSource::TyreContactHeadingZFr => records
            .iter()
            .map(|r| r.tyre_contact_heading.front_right.z)
            .collect(),
        ChannelSource::TyreContactHeadingXRl => records
            .iter()
            .map(|r| r.tyre_contact_heading.rear_left.x)
            .collect(),
        ChannelSource::TyreContactHeadingYRl => records
            .iter()
            .map(|r| r.tyre_contact_heading.rear_left.y)
            .collect(),
        ChannelSource::TyreContactHeadingZRl => records
            .iter()
            .map(|r| r.tyre_contact_heading.rear_left.z)
            .collect(),
        ChannelSource::TyreContactHeadingXRr => records
            .iter()
            .map(|r| r.tyre_contact_heading.rear_right.x)
            .collect(),
        ChannelSource::TyreContactHeadingYRr => records
            .iter()
            .map(|r| r.tyre_contact_heading.rear_right.y)
            .collect(),
        ChannelSource::TyreContactHeadingZRr => records
            .iter()
            .map(|r| r.tyre_contact_heading.rear_right.z)
            .collect(),
        ChannelSource::VelocityX => records.iter().map(|r| r.velocity.x).collect(),
        ChannelSource::VelocityY => records.iter().map(|r| r.velocity.y).collect(),
        ChannelSource::VelocityZ => records.iter().map(|r| r.velocity.z).collect(),
        ChannelSource::LocalVelocityX => records.iter().map(|r| r.local_velocity.x).collect(),
        ChannelSource::LocalVelocityY => records.iter().map(|r| r.local_velocity.y).collect(),
        ChannelSource::LocalVelocityZ => records.iter().map(|r| r.local_velocity.z).collect(),
        ChannelSource::LocalAngularVelX => records.iter().map(|r| r.local_angular_vel.x).collect(),
        ChannelSource::LocalAngularVelY => records.iter().map(|r| r.local_angular_vel.y).collect(),
        ChannelSource::LocalAngularVelZ => records.iter().map(|r| r.local_angular_vel.z).collect(),
        ChannelSource::FinalFf => records.iter().map(|r| r.final_ff).collect(),
        ChannelSource::BrakeBias => records.iter().map(|r| r.brake_bias).collect(),
        ChannelSource::TcInAction => records
            .iter()
            .map(|r| if r.tc_in_action { 1.0 } else { 0.0 })
            .collect(),
        ChannelSource::AbsInAction => records
            .iter()
            .map(|r| if r.abs_in_action { 1.0 } else { 0.0 })
            .collect(),
        ChannelSource::PitLimiterOn => records
            .iter()
            .map(|r| if r.pit_limiter_on { 1.0 } else { 0.0 })
            .collect(),
        ChannelSource::IsAiControlled => records
            .iter()
            .map(|r| if r.is_ai_controlled { 1.0 } else { 0.0 })
            .collect(),
        ChannelSource::Drs => records.iter().map(|r| r.drs as f32).collect(),
        ChannelSource::CgHeight => records.iter().map(|r| r.cg_height).collect(),
        ChannelSource::NumberOfTyresOut => records
            .iter()
            .map(|r| r.number_of_tyres_out as f32)
            .collect(),
        ChannelSource::KersCharge => records.iter().map(|r| r.kers_charge).collect(),
        ChannelSource::KersInput => records.iter().map(|r| r.kers_input).collect(),
        ChannelSource::KersCurrentKj => records.iter().map(|r| r.kers_current_kj).collect(),
        ChannelSource::Ballast => records.iter().map(|r| r.ballast).collect(),
        ChannelSource::AirDensity => records.iter().map(|r| r.air_density).collect(),
        ChannelSource::PerformanceMeter => records.iter().map(|r| r.performance_meter).collect(),
        ChannelSource::EngineBrake => records.iter().map(|r| r.engine_brake as f32).collect(),
        ChannelSource::ErsRecoveryLevel => records
            .iter()
            .map(|r| r.ers_recovery_level as f32)
            .collect(),
        ChannelSource::ErsPowerLevel => records.iter().map(|r| r.ers_power_level as f32).collect(),
        ChannelSource::ErsHeatCharging => records
            .iter()
            .map(|r| r.ers_heat_charging as f32)
            .collect(),
        ChannelSource::ErsIsCharging => records.iter().map(|r| r.ers_is_charging as f32).collect(),
        ChannelSource::DrsAvailable => records.iter().map(|r| r.drs_available as f32).collect(),
        ChannelSource::DrsEnabled => records.iter().map(|r| r.drs_enabled as f32).collect(),
        ChannelSource::P2pActivation => records.iter().map(|r| r.p2p_activation as f32).collect(),
        ChannelSource::P2pStatus => records.iter().map(|r| r.p2p_status as f32).collect(),
        ChannelSource::CurrentMaxRpm => records.iter().map(|r| r.current_max_rpm as f32).collect(),
        ChannelSource::MzFl => records.iter().map(|r| r.mz.front_left).collect(),
        ChannelSource::MzFr => records.iter().map(|r| r.mz.front_right).collect(),
        ChannelSource::MzRl => records.iter().map(|r| r.mz.rear_left).collect(),
        ChannelSource::MzRr => records.iter().map(|r| r.mz.rear_right).collect(),
        ChannelSource::FzFl => records.iter().map(|r| r.fz.front_left).collect(),
        ChannelSource::FzFr => records.iter().map(|r| r.fz.front_right).collect(),
        ChannelSource::FzRl => records.iter().map(|r| r.fz.rear_left).collect(),
        ChannelSource::FzRr => records.iter().map(|r| r.fz.rear_right).collect(),
        ChannelSource::MyFl => records.iter().map(|r| r.my.front_left).collect(),
        ChannelSource::MyFr => records.iter().map(|r| r.my.front_right).collect(),
        ChannelSource::MyRl => records.iter().map(|r| r.my.rear_left).collect(),
        ChannelSource::MyRr => records.iter().map(|r| r.my.rear_right).collect(),
        ChannelSource::KerbVibration => records.iter().map(|r| r.kerb_vibration).collect(),
        ChannelSource::SlipVibration => records.iter().map(|r| r.slip_vibration).collect(),
        ChannelSource::GVibration => records.iter().map(|r| r.g_vibration).collect(),
        ChannelSource::AbsVibration => records.iter().map(|r| r.abs_vibration).collect(),
        ChannelSource::AutoshifterOn => records
            .iter()
            .map(|r| if r.autoshifter_on { 1.0 } else { 0.0 })
            .collect(),
        ChannelSource::IgnitionOn => records
            .iter()
            .map(|r| if r.ignition_on { 1.0 } else { 0.0 })
            .collect(),
        ChannelSource::StarterEngineOn => records
            .iter()
            .map(|r| if r.starter_engine_on { 1.0 } else { 0.0 })
            .collect(),
        ChannelSource::IsEngineRunning => records
            .iter()
            .map(|r| if r.is_engine_running { 1.0 } else { 0.0 })
            .collect(),
        ChannelSource::PacketId => records.iter().map(|r| r.packet_id as f32).collect(),
    })
}

fn resample_graphics_to_len(
    graphics: &[GraphicsRecord],
    target_len: usize,
    getter: impl Fn(&GraphicsRecord) -> f32,
) -> Vec<f32> {
    if target_len == 0 || graphics.is_empty() {
        return Vec::new();
    }
    if target_len == 1 {
        return vec![getter(&graphics[0])];
    }
    if graphics.len() == 1 {
        return vec![getter(&graphics[0]); target_len];
    }
    (0..target_len)
        .map(|i| {
            let src_idx = i * (graphics.len() - 1) / (target_len - 1);
            getter(&graphics[src_idx])
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn loads_builtin_rally_and_rbr() {
        for id in ["rally", "rbr", "all_data"] {
            let p = load_profile(id, None).expect(id);
            assert!(!p.channels.is_empty(), "{id}");
        }
    }

    #[test]
    fn motec_time_secs_from_capture_timestamps() {
        let mut a = minimal_physics_record();
        a.capture_time_sec = 1.0;
        let mut b = minimal_physics_record();
        b.capture_time_sec = 1.01;
        let mut c = minimal_physics_record();
        c.capture_time_sec = 1.05;
        let t = PhysicsRecord::motec_time_secs(&[a, b, c]);
        assert!((t[0] - 0.0).abs() < 1e-6);
        assert!((t[1] - 0.01).abs() < 1e-6);
        assert!((t[2] - 0.05).abs() < 1e-6);
    }

    fn minimal_physics_record() -> PhysicsRecord {
        use crate::record::{
            CarDamageRecord, ContactPointRecord, PhysicsRecord, Vector3fRecord, WheelsRecord,
        };
        let z = 0.0_f32;
        let w = WheelsRecord {
            front_left: z,
            front_right: z,
            rear_left: z,
            rear_right: z,
        };
        let c = ContactPointRecord {
            front_left: Vector3fRecord { x: z, y: z, z },
            front_right: Vector3fRecord { x: z, y: z, z },
            rear_left: Vector3fRecord { x: z, y: z, z },
            rear_right: Vector3fRecord { x: z, y: z, z },
        };
        PhysicsRecord {
            packet_id: 0,
            gas: z,
            brake: z,
            clutch: z,
            steer_angle: z,
            gear: 0,
            rpm: 0,
            autoshifter_on: false,
            ignition_on: false,
            starter_engine_on: false,
            is_engine_running: false,
            speed_kmh: z,
            velocity: Vector3fRecord { x: z, y: z, z },
            local_velocity: Vector3fRecord { x: z, y: z, z },
            local_angular_vel: Vector3fRecord { x: z, y: z, z },
            g_force: Vector3fRecord { x: z, y: z, z },
            heading: z,
            pitch: z,
            roll: z,
            final_ff: z,
            wheel_slip: w.clone(),
            wheel_load: w.clone(),
            wheel_pressure: w.clone(),
            wheel_angular_speed: w.clone(),
            tyre_wear: w.clone(),
            tyre_dirty_level: w.clone(),
            tyre_core_temp: w.clone(),
            camber_rad: w.clone(),
            suspension_travel: w.clone(),
            brake_temp: w.clone(),
            brake_pressure: w.clone(),
            suspension_damage: w.clone(),
            slip_ratio: w.clone(),
            slip_angle: w.clone(),
            pad_life: w.clone(),
            disc_life: w.clone(),
            front_brake_compound: 0,
            rear_brake_compound: 0,
            tyre_temp_i: w.clone(),
            tyre_temp_m: w.clone(),
            tyre_temp_o: w.clone(),
            tyre_temp_extra: w.clone(),
            tyre_contact_point: c.clone(),
            tyre_contact_normal: c.clone(),
            tyre_contact_heading: c,
            fuel: z,
            tc: z,
            abs: z,
            pit_limiter_on: false,
            turbo_boost: z,
            air_temp: z,
            road_temp: z,
            water_temp: z,
            car_damage: CarDamageRecord {
                front: z,
                rear: z,
                left: z,
                right: z,
                center: z,
            },
            is_ai_controlled: false,
            brake_bias: z,
            tc_in_action: false,
            abs_in_action: false,
            drs: 0,
            cg_height: z,
            number_of_tyres_out: 0,
            kers_charge: z,
            kers_input: z,
            ride_height_front: z,
            ride_height_rear: z,
            ballast: z,
            air_density: z,
            performance_meter: z,
            engine_brake: 0,
            ers_recovery_level: 0,
            ers_power_level: 0,
            ers_heat_charging: 0,
            ers_is_charging: 0,
            kers_current_kj: z,
            drs_available: 0,
            drs_enabled: 0,
            p2p_activation: 0,
            p2p_status: 0,
            current_max_rpm: 0,
            mz: w.clone(),
            fz: w.clone(),
            my: w,
            kerb_vibration: z,
            slip_vibration: z,
            g_vibration: z,
            abs_vibration: z,
            capture_time_sec: 0.0,
        }
    }
}
