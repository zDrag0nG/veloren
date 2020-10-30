use crate::{
    client::Client,
    presence::RegionSubscription,
    streams::{
        CharacterScreenStream, GeneralStream, GetStream, InGameStream, PingStream, RegisterStream,
    },
    Server,
};
use common::{
    comp::{self, item, Pos},
    consts::MAX_MOUNT_RANGE,
    msg::ServerGeneral,
    sync::{Uid, WorldSyncExt},
};
use specs::{world::WorldExt, Entity as EcsEntity};
use tracing::error;

pub fn handle_lantern(server: &mut Server, entity: EcsEntity, enable: bool) {
    let ecs = server.state_mut().ecs();

    let lantern_exists = ecs
        .read_storage::<comp::LightEmitter>()
        .get(entity)
        .map_or(false, |light| light.strength > 0.0);

    if lantern_exists != enable {
        if !enable {
            server
                .state_mut()
                .ecs()
                .write_storage::<comp::LightEmitter>()
                .remove(entity);
        } else {
            let loadout_storage = ecs.read_storage::<comp::Loadout>();
            let lantern_opt = loadout_storage
                .get(entity)
                .and_then(|loadout| loadout.lantern.as_ref())
                .and_then(|item| {
                    if let comp::item::ItemKind::Lantern(l) = item.kind() {
                        Some(l)
                    } else {
                        None
                    }
                });
            if let Some(lantern) = lantern_opt {
                let _ =
                    ecs.write_storage::<comp::LightEmitter>()
                        .insert(entity, comp::LightEmitter {
                            col: lantern.color(),
                            strength: lantern.strength(),
                            flicker: 0.35,
                            animated: true,
                        });
            }
        }
    }
}

pub fn handle_mount(server: &mut Server, mounter: EcsEntity, mountee: EcsEntity) {
    let state = server.state_mut();

    if state
        .ecs()
        .read_storage::<comp::Mounting>()
        .get(mounter)
        .is_none()
    {
        let not_mounting_yet = matches!(
            state.ecs().read_storage::<comp::MountState>().get(mountee),
            Some(comp::MountState::Unmounted)
        );

        let within_range = within_mounting_range(
            state.ecs().read_storage::<comp::Pos>().get(mounter),
            state.ecs().read_storage::<comp::Pos>().get(mountee),
        );

        if not_mounting_yet && within_range {
            if let (Some(mounter_uid), Some(mountee_uid)) = (
                state.ecs().uid_from_entity(mounter),
                state.ecs().uid_from_entity(mountee),
            ) {
                state.write_component(mountee, comp::MountState::MountedBy(mounter_uid));
                state.write_component(mounter, comp::Mounting(mountee_uid));
            }
        }
    }
}

pub fn handle_unmount(server: &mut Server, mounter: EcsEntity) {
    let state = server.state_mut();
    let mountee_entity = state
        .ecs()
        .write_storage::<comp::Mounting>()
        .get(mounter)
        .and_then(|mountee| state.ecs().entity_from_uid(mountee.0.into()));
    if let Some(mountee_entity) = mountee_entity {
        state
            .ecs()
            .write_storage::<comp::MountState>()
            .get_mut(mountee_entity)
            .map(|ms| *ms = comp::MountState::Unmounted);
    }
    state.delete_component::<comp::Mounting>(mounter);
}

#[allow(clippy::nonminimal_bool)] // TODO: Pending review in #587
pub fn handle_possess(server: &Server, possessor_uid: Uid, possesse_uid: Uid) {
    let state = &server.state;
    let ecs = state.ecs();
    if let (Some(possessor), Some(possesse)) = (
        ecs.entity_from_uid(possessor_uid.into()),
        ecs.entity_from_uid(possesse_uid.into()),
    ) {
        // Check that entities still exist
        if !(possessor.gen().is_alive() && ecs.is_alive(possessor))
            || !(possesse.gen().is_alive() && ecs.is_alive(possesse))
        {
            error!(
                "Error possessing! either the possessor entity or possesse entity no longer exists"
            );
            return;
        }

        let mut clients = ecs.write_storage::<Client>();
        let mut general_streams = ecs.write_storage::<GeneralStream>();

        if clients.get_mut(possesse).is_some() {
            error!("can't possess other players");
            return;
        }

        match (|| -> Option<Result<(), specs::error::Error>> {
            let mut ping_streams = ecs.write_storage::<PingStream>();
            let mut register_streams = ecs.write_storage::<RegisterStream>();
            let mut character_screen_streams = ecs.write_storage::<CharacterScreenStream>();
            let mut in_game_streams = ecs.write_storage::<InGameStream>();

            let c = clients.remove(possessor)?;
            clients.insert(possesse, c).ok()?;
            let s = general_streams.remove(possessor)?;
            general_streams.insert(possesse, s).ok()?;
            let s = ping_streams.remove(possessor)?;
            ping_streams.insert(possesse, s).ok()?;
            let s = register_streams.remove(possessor)?;
            register_streams.insert(possesse, s).ok()?;
            let s = character_screen_streams.remove(possessor)?;
            character_screen_streams.insert(possesse, s).ok()?;
            let s = in_game_streams.remove(possessor)?;
            in_game_streams.insert(possesse, s).ok()?;
            //optional entities
            let mut players = ecs.write_storage::<comp::Player>();
            let mut subscriptions = ecs.write_storage::<RegionSubscription>();
            let mut admins = ecs.write_storage::<comp::Admin>();
            let mut waypoints = ecs.write_storage::<comp::Waypoint>();
            players
                .remove(possessor)
                .map(|p| players.insert(possesse, p).ok()?);
            subscriptions
                .remove(possessor)
                .map(|s| subscriptions.insert(possesse, s).ok()?);
            admins
                .remove(possessor)
                .map(|a| admins.insert(possesse, a).ok()?);
            waypoints
                .remove(possessor)
                .map(|w| waypoints.insert(possesse, w).ok()?);

            Some(Ok(()))
        })() {
            Some(Ok(())) => (),
            Some(Err(e)) => {
                error!(?e, ?possesse, "Error inserting component during possession");
                return;
            },
            None => {
                error!(?possessor, "Error removing component during possession");
                return;
            },
        }

        general_streams
            .get_mut(possesse)
            .map(|s| s.send_fallible(ServerGeneral::SetPlayerEntity(possesse_uid)));

        // Put possess item into loadout
        let mut loadouts = ecs.write_storage::<comp::Loadout>();
        let loadout = loadouts
            .entry(possesse)
            .expect("Could not read loadouts component while possessing")
            .or_insert(comp::Loadout::default());

        let item = comp::Item::new_from_asset_expect("common.items.debug.possess");
        if let item::ItemKind::Tool(tool) = item.kind() {
            let mut abilities = tool.get_abilities();
            let mut ability_drain = abilities.drain(..);
            let debug_item = comp::ItemConfig {
                item,
                ability1: ability_drain.next(),
                ability2: ability_drain.next(),
                ability3: ability_drain.next(),
                block_ability: None,
                dodge_ability: None,
            };
            std::mem::swap(&mut loadout.active_item, &mut loadout.second_item);
            loadout.active_item = Some(debug_item);
        }

        // Remove will of the entity
        ecs.write_storage::<comp::Agent>().remove(possesse);
        // Reset controller of former shell
        ecs.write_storage::<comp::Controller>()
            .get_mut(possessor)
            .map(|c| c.reset());
    }
}

fn within_mounting_range(player_position: Option<&Pos>, mount_position: Option<&Pos>) -> bool {
    match (player_position, mount_position) {
        (Some(ppos), Some(ipos)) => ppos.0.distance_squared(ipos.0) < MAX_MOUNT_RANGE.powi(2),
        _ => false,
    }
}
