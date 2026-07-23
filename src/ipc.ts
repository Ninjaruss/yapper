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
}

export interface SessionStatus {
  id: number;
  state: "recording" | "paused" | "ended";
  elapsed_ms: number;
  writer_failed: boolean;
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
  onLevel: (cb: (level: number) => void): Promise<UnlistenFn> =>
    listen<number>("audio:level", (e) => cb(e.payload)),
};
