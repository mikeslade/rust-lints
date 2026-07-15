mod sqlx {
    pub fn query<T>(_sql: T) {}

    pub fn query_scalar<T>(_sql: T) {}
}

pub struct AssertSqlSafe<T>(pub T);

pub fn static_query_without_macro() {
    sqlx::query("SELECT id FROM workers");
}

pub fn static_scalar_without_macro() {
    sqlx::query_scalar::<&str>("SELECT COUNT(*) FROM elections");
}

pub fn dynamic_query_without_safety_note(table: &str) {
    let sql = format!("select * from {table}");
    sqlx::query(sql.as_str());
}

pub fn inline_sql_without_marker() -> &'static str {
    "SELECT id FROM deductions"
}
