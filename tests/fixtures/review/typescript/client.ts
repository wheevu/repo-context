export type Session = { userId: string };

export function refreshToken(input: string): string {
  return "token:" + input;
}
