/** An expired-session event can arrive after a different account has signed in. */
export async function shouldApplyAuthExpired(
  readStatus: () => Promise<{ authenticated: boolean; unavailable?: boolean }>,
  stillCurrent: () => boolean,
): Promise<boolean> {
  try {
    const status = await readStatus();
    return stillCurrent() && status.authenticated === false && !status.unavailable;
  } catch {
    // A failed recheck cannot prove that the currently visible login is expired.
    return false;
  }
}
