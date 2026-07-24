import "./styles.css";
import "./wisp.css";
import { renderSetup } from "./screens/setup";
import { renderLive } from "./screens/live";
import { renderRecap } from "./screens/recap";
import { ipc, type Session } from "./ipc";

const root = document.getElementById("app")!;

function showSetup() {
  renderSetup(root, showLive);
}

async function showLive() {
  renderLive(root, async (endedSession: Session) => {
    // end_session's Session never computes segment_count (it's always 0
    // there — only list_sessions fills it in), so refresh from
    // listSessions for an accurate Export-transcript gate. Fall back to
    // the session end_session gave us if that refresh fails.
    let session = endedSession;
    try {
      const sessions = await ipc.listSessions();
      const fresh = sessions.find((s) => s.id === endedSession.id);
      if (fresh) session = fresh;
    } catch {
      // stale segment_count is a minor cosmetic issue, not worth blocking recap over
    }
    renderRecap(root, session, showSetup);
  });
}

showSetup();
