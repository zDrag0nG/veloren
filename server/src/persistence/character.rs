extern crate diesel;

use super::{
    error::Error,
    establish_connection,
    models::{Body, Character, NewCharacter, Stats, StatsJoinData},
    schema,
};
use crate::comp;
use common::character::{Character as CharacterData, CharacterItem, MAX_CHARACTERS_PER_PLAYER};
use diesel::prelude::*;

type CharacterListResult = Result<Vec<CharacterItem>, Error>;

// Loading of characters happens immediately after login, and the data is only
// for the purpose of rendering the character and their level in the character
// list.
pub fn load_characters(player_uuid: &str) -> CharacterListResult {
    let data: Vec<(Character, Body, Stats)> = schema::character::dsl::character
        .filter(schema::character::player_uuid.eq(player_uuid))
        .order(schema::character::id.desc())
        .inner_join(schema::body::table)
        .inner_join(schema::stats::table)
        .load::<(Character, Body, Stats)>(&establish_connection())?;

    Ok(data
        .iter()
        .map(|(character_data, body_data, stats_data)| {
            let character = CharacterData::from(character_data);
            let body = comp::Body::from(body_data);
            let stats = comp::Stats::from(StatsJoinData {
                character: &character,
                body: &body,
                stats: stats_data,
            });

            CharacterItem {
                character,
                body,
                stats,
            }
        })
        .collect())
}

/// Create a new character with provided comp::Character and comp::Body data.
/// Note that sqlite does not support returning the inserted data after a
/// successful insert. To workaround, we wrap this in a transaction which
/// inserts, queries for the newly created chaacter id, then uses the character
/// id for insertion of the `body` table entry
pub fn create_character(
    uuid: &str,
    character_alias: String,
    character_tool: Option<String>,
    body: &comp::Body,
) -> CharacterListResult {
    check_character_limit(uuid)?;

    let connection = establish_connection();

    connection.transaction::<_, diesel::result::Error, _>(|| {
        use schema::{body, character, character::dsl::*, stats};

        match body {
            comp::Body::Humanoid(body_data) => {
                let new_character = NewCharacter {
                    player_uuid: uuid,
                    alias: &character_alias,
                    tool: character_tool.as_deref(),
                };

                diesel::insert_into(character::table)
                    .values(&new_character)
                    .execute(&connection)?;

                let inserted_character = character
                    .filter(player_uuid.eq(uuid))
                    .order(id.desc())
                    .first::<Character>(&connection)?;

                let new_body = Body {
                    character_id: inserted_character.id as i32,
                    race: body_data.race as i16,
                    body_type: body_data.body_type as i16,
                    hair_style: body_data.hair_style as i16,
                    beard: body_data.beard as i16,
                    eyebrows: body_data.eyebrows as i16,
                    accessory: body_data.accessory as i16,
                    hair_color: body_data.hair_color as i16,
                    skin: body_data.skin as i16,
                    eye_color: body_data.eye_color as i16,
                };

                diesel::insert_into(body::table)
                    .values(&new_body)
                    .execute(&connection)?;

                let default_stats = comp::Stats::new(String::from(new_character.alias), *body);

                // Insert some default stats
                let new_stats = Stats {
                    character_id: inserted_character.id as i32,
                    level: default_stats.level.level() as i32,
                    exp: default_stats.exp.current() as i32,
                    endurance: default_stats.endurance as i32,
                    fitness: default_stats.fitness as i32,
                    willpower: default_stats.willpower as i32,
                };

                diesel::insert_into(stats::table)
                    .values(&new_stats)
                    .execute(&connection)?;
            },
            _ => log::warn!("Creating non-humanoid characters is not supported."),
        };

        Ok(())
    })?;

    load_characters(uuid)
}

pub fn delete_character(uuid: &str, character_id: i32) -> CharacterListResult {
    use schema::character::dsl::*;

    diesel::delete(character.filter(id.eq(character_id))).execute(&establish_connection())?;

    load_characters(uuid)
}

fn check_character_limit(uuid: &str) -> Result<(), Error> {
    use diesel::dsl::count_star;
    use schema::character::dsl::*;

    let character_count = character
        .select(count_star())
        .filter(player_uuid.eq(uuid))
        .load::<i64>(&establish_connection())?;

    match character_count.first() {
        Some(count) => {
            if count < &(MAX_CHARACTERS_PER_PLAYER as i64) {
                Ok(())
            } else {
                Err(Error::CharacterLimitReached)
            }
        },
        _ => Ok(()),
    }
}
