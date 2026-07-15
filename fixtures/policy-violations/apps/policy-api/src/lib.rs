mod sqlx {
    pub fn query(_sql: &str) {}
}

pub fn app_level_sql() {
    sqlx::query("SELECT id FROM workers");
}
