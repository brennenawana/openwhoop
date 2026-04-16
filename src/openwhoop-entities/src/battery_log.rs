use sea_orm::entity::prelude::*;

#[derive(Clone, Debug, PartialEq, DeriveEntityModel)]
#[sea_orm(table_name = "battery_log")]
pub struct Model {
    #[sea_orm(primary_key)]
    pub id: i32,
    pub time: DateTime,
    #[sea_orm(column_type = "Double")]
    pub percent: f64,
    pub charging: bool,
    pub is_worn: bool,
    pub avg_bpm: Option<i16>,
}

#[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
pub enum Relation {}

impl ActiveModelBehavior for ActiveModel {}
