class Session:
    def __init__(self, user_id: str):
        self.user_id = user_id


def refresh_token(input: str) -> str:
    return f"token:{input}"
