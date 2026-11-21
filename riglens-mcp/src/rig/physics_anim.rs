use glam::Vec2;
use riglens_core::animation::{AnimationLane, Keyframe, PuppetAnimation};
use riglens_core::math::interp::InterpolateMode;
use riglens_core::math::transform::TransformOffset;
use riglens_core::node::components::{PhysicsModel, PhysicsParamMapMode, PhysicsProps, SimplePhysics};
use riglens_core::node::{InoxNode, InoxNodeUuid};
use riglens_core::puppet::Puppet;

/// Attach a `SimplePhysics` pendulum driver node under `parent` targeting `param_name`.
pub fn add_pendulum_physics(
	puppet: &mut Puppet,
	parent: InoxNodeUuid,
	name: impl Into<String>,
	param_name: &str,
	model_type: PhysicsModel,
	length: f32,
	frequency: f32,
	angle_damping: f32,
	output_scale: Vec2,
) -> Result<InoxNodeUuid, String> {
	let param_uuid = puppet
		.params
		.get(param_name)
		.map(|p| p.uuid)
		.ok_or_else(|| format!("Parameter '{param_name}' does not exist"))?;

	let id = puppet.next_node_uuid();
	puppet.add_node(
		parent,
		InoxNode {
			uuid: id,
			name: name.into(),
			enabled: true,
			zsort: 0.0,
			trans_offset: TransformOffset::default(),
			lock_to_root: false,
		},
	);
	puppet.node_comps.add(
		id,
		SimplePhysics {
			param: param_uuid,
			model_type,
			map_mode: PhysicsParamMapMode::XY,
			props: PhysicsProps {
				gravity: 1.0,
				length,
				frequency,
				angle_damping,
				length_damping: 0.5,
				output_scale,
			},
			local_only: false,
		},
	);
	Ok(id)
}

/// Create a 120-frame @ 30fps looping `"Idle"` animation driving breathing, blinking, and gentle head sway.
pub fn create_idle_animation(puppet: &mut Puppet) -> String {
	let mut lanes = Vec::new();

	if let Some(p) = puppet.params.get("Breath") {
		lanes.push(AnimationLane {
			interpolation: InterpolateMode::Linear,
			param_uuid: p.uuid,
			target: 0,
			merge_mode: "Additive".to_owned(),
			keyframes: vec![
				Keyframe { frame: 0, value: 0.0, tension: 0.6 },
				Keyframe { frame: 60, value: 1.0, tension: 0.6 },
				Keyframe { frame: 120, value: 0.0, tension: 0.6 },
			],
		});
	}

	if let Some(p) = puppet.params.get("Head:: Yaw-Pitch") {
		lanes.push(AnimationLane {
			interpolation: InterpolateMode::Linear,
			param_uuid: p.uuid,
			target: 0,
			merge_mode: "Additive".to_owned(),
			keyframes: vec![
				Keyframe { frame: 0, value: 0.0, tension: 0.6 },
				Keyframe { frame: 30, value: 0.35, tension: 0.6 },
				Keyframe { frame: 60, value: 0.0, tension: 0.6 },
				Keyframe { frame: 90, value: -0.35, tension: 0.6 },
				Keyframe { frame: 120, value: 0.0, tension: 0.6 },
			],
		});
		lanes.push(AnimationLane {
			interpolation: InterpolateMode::Linear,
			param_uuid: p.uuid,
			target: 1,
			merge_mode: "Additive".to_owned(),
			keyframes: vec![
				Keyframe { frame: 0, value: 0.0, tension: 0.6 },
				Keyframe { frame: 45, value: -0.20, tension: 0.6 },
				Keyframe { frame: 90, value: 0.20, tension: 0.6 },
				Keyframe { frame: 120, value: 0.0, tension: 0.6 },
			],
		});
	}

	for eye_param in ["Eye:: Left:: Open", "Eye:: Right:: Open"] {
		if let Some(p) = puppet.params.get(eye_param) {
			lanes.push(AnimationLane {
				interpolation: InterpolateMode::Linear,
				param_uuid: p.uuid,
				target: 0,
				merge_mode: "Additive".to_owned(),
				keyframes: vec![
					Keyframe { frame: 0, value: 1.0, tension: 0.5 },
					Keyframe { frame: 46, value: 1.0, tension: 0.5 },
					Keyframe { frame: 50, value: 0.0, tension: 0.5 },
					Keyframe { frame: 54, value: 1.0, tension: 0.5 },
					Keyframe { frame: 120, value: 1.0, tension: 0.5 },
				],
			});
		}
	}

	puppet.animations.insert(
		"Idle".to_owned(),
		PuppetAnimation {
			timestep: 1.0 / 30.0,
			additive: false,
			length: 120,
			lead_in: 0,
			lead_out: 0,
			lanes,
		},
	);
	"Idle".to_owned()
}

/// Configure an existing `SimplePhysics` pendulum node.
pub fn configure_pendulum_physics(
	puppet: &mut Puppet,
	node_id: InoxNodeUuid,
	model_type: Option<PhysicsModel>,
	length: Option<f32>,
	frequency: Option<f32>,
	angle_damping: Option<f32>,
	length_damping: Option<f32>,
	gravity: Option<f32>,
	output_scale: Option<Vec2>,
) -> Result<(), String> {
	let phys = puppet
		.node_comps
		.get_mut::<SimplePhysics>(node_id)
		.ok_or_else(|| format!("Node {} does not have a SimplePhysics component", node_id.0))?;

	if let Some(mt) = model_type {
		phys.model_type = mt;
	}
	if let Some(l) = length {
		phys.props.length = l.clamp(5.0, 800.0);
	}
	if let Some(f) = frequency {
		phys.props.frequency = f.clamp(0.1, 20.0);
	}
	if let Some(ad) = angle_damping {
		phys.props.angle_damping = ad.clamp(0.0, 10.0);
	}
	if let Some(ld) = length_damping {
		phys.props.length_damping = ld.clamp(0.0, 10.0);
	}
	if let Some(g) = gravity {
		phys.props.gravity = g.clamp(-5.0, 10.0);
	}
	if let Some(os) = output_scale {
		phys.props.output_scale = os;
	}
	puppet.reset_contexts();
	Ok(())
}

/// Insert or update a keyframe on `anim_name` for `(param_name, target)` (creating the lane/animation if needed).
pub fn set_animation_keyframe(
	puppet: &mut Puppet,
	anim_name: &str,
	param_name: &str,
	target: u8,
	frame: i32,
	value: f32,
	tension: f32,
	interpolation: Option<InterpolateMode>,
) -> Result<(), String> {
	let param_uuid = puppet
		.params
		.get(param_name)
		.map(|p| p.uuid)
		.ok_or_else(|| format!("Parameter '{param_name}' does not exist"))?;

	let anim = puppet.animations.entry(anim_name.to_owned()).or_insert_with(|| PuppetAnimation {
		timestep: 1.0 / 30.0,
		additive: false,
		length: 120,
		lead_in: 0,
		lead_out: 0,
		lanes: Vec::new(),
	});
	let frame = frame.max(0);
	if frame + 1 > anim.length {
		anim.length = frame + 1;
	}

	let lane = if let Some(existing) = anim
		.lanes
		.iter_mut()
		.find(|l| l.param_uuid == param_uuid && l.target == target)
	{
		existing
	} else {
		anim.lanes.push(AnimationLane {
			interpolation: interpolation.unwrap_or(InterpolateMode::Linear),
			param_uuid,
			target,
			keyframes: Vec::new(),
			merge_mode: "Forced".to_owned(),
		});
		anim.lanes.last_mut().unwrap()
	};

	if let Some(mode) = interpolation {
		lane.interpolation = mode;
	}

	let kf = Keyframe {
		frame,
		value,
		tension: tension.clamp(0.0, 1.0),
	};
	if let Some(pos) = lane.keyframes.iter().position(|k| k.frame == frame) {
		lane.keyframes[pos] = kf;
	} else {
		lane.keyframes.push(kf);
		lane.keyframes.sort_by_key(|k| k.frame);
	}
	Ok(())
}

/// Remove a keyframe at `frame` from `(anim_name, param_name, target)`. Returns true if removed.
pub fn remove_animation_keyframe(
	puppet: &mut Puppet,
	anim_name: &str,
	param_name: &str,
	target: u8,
	frame: i32,
) -> Result<bool, String> {
	let param_uuid = puppet
		.params
		.get(param_name)
		.map(|p| p.uuid)
		.ok_or_else(|| format!("Parameter '{param_name}' does not exist"))?;

	let Some(anim) = puppet.animations.get_mut(anim_name) else {
		return Err(format!("Animation '{anim_name}' not found"));
	};
	let Some(lane) = anim
		.lanes
		.iter_mut()
		.find(|l| l.param_uuid == param_uuid && l.target == target)
	else {
		return Ok(false);
	};
	let before = lane.keyframes.len();
	lane.keyframes.retain(|k| k.frame != frame);
	Ok(lane.keyframes.len() < before)
}
