use phoenix::database::Model;

#[derive(Debug, Model)]
pub struct Note {
    #[key]
    #[auto]
    pub id: u64,
    pub name: String,
}
