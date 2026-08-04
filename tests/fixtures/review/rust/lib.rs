pub fn refresh_token(input: &str) -> String {
    format!("token:{input}")
}

pub struct Session {
    pub user_id: String,
}
