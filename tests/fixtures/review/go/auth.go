package auth

type Session struct {
	UserID string
}

func RefreshToken(input string) string {
	return "token:" + input
}
