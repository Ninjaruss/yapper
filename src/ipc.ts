import { invoke } from "@tauri-apps/api/core";
import { listen, type UnlistenFn } from "@tauri-apps/api/event";

export interface InputDevice {
  name: string;
  is_default: boolean;
}

export interface Session {
  id: number;
  started_at_ms: number;
  ended_at_ms: number | null;
  intent: string;
  audio_path: string | null;
  duration_ms: number | null;
  paused_ms: number;
  audio_exists: boolean;
}

export interface SessionStatus {
  id: number;
  state: "recording" | "paused" | "ended";
  elapsed_ms: number;
  writer_failed: boolean;
  stt_active: boolean;
  stt_failed: boolean;
}

export interface TranscriptSegment {
  id: number;
  session_id: number;
  start_ms: number;
  end_ms: number;
  text: string;
}

export interface Segment {
  start_ms: number;
  end_ms: number;
  text: string;
}

export interface ModelProgress {
  downloaded: number;
  total: number;
}

export const ipc = {
  listInputDevices: () => invoke<InputDevice[]>("list_input_devices"),
  startSession: (intent: string, deviceName?: string) =>
    invoke<number>("start_session", { intent, deviceName: deviceName ?? null }),
  pauseListening: () => invoke<void>("pause_listening"),
  resumeListening: () => invoke<void>("resume_listening"),
  sessionStatus: () => invoke<SessionStatus | null>("session_status"),
  endSession: () => invoke<Session>("end_session"),
  listSessions: () => invoke<Session[]>("list_sessions"),
  revealSession: (id: number) => invoke<void>("reveal_session", { id }),
  forgetSession: (id: number) => invoke<void>("forget_session", { id }),
  listSegments: (sessionId: number) =>
    invoke<TranscriptSegment[]>("list_segments", { sessionId }),
  modelsReady: () => invoke<boolean>("models_ready"),
  ensureModels: () => invoke<void>("ensure_models"),
  onLevel: (cb: (level: number) => void): Promise<UnlistenFn> =>
    listen<number>("audio:level", (e) => cb(e.payload)),
  onSegment: (cb: (s: Segment) => void): Promise<UnlistenFn> =>
    listen<Segment>("transcript:segment", (e) => cb(e.payload)),
  onModelProgress: (cb: (p: ModelProgress) => void): Promise<UnlistenFn> =>
    listen<ModelProgress>("model:progress", (e) => cb(e.payload)),
};
