// Response shapes used by the local reader. Keep aligned with the existing native IPC.
// Type declarations only; this module has no runtime dependencies.

export type SessionItem = {
  username: string;
  display: string;
  last_time: number | null;
  is_group: boolean;
  member_count: number | null;
  preview: string | null;
  unread: number | null;
};

export type MessageItem = {
  local_id: number;
  source_shard: string;
  time: number;
  sender: string | null;
  sender_name: string | null;
  kind: number;
  text: string;
  is_self: boolean;
};

export type MediaItem = {
  local_id: number;
  server_id: number;
  server_id_exact: string;
  source_shard: string;
  time: number;
  kind: 'image' | 'video' | 'file' | 'voice' | 'emoji' | string;
  name: string | null;
  rel_path: string | null;
  size: number | null;
  exists: boolean;
  encrypted: boolean;
  decoded: boolean;
  thumb: string | null;
  preview: string | null;
  sources: string[];
  duration: number | null;
  note: string | null;
};

export type MediaReport = {
  items: MediaItem[];
  counts: { images: number; videos: number; files: number; voices: number; encrypted: number; missing: number };
};

export type VoiceResult = {
  server_id: number;
  local_id: number;
  wav: string | null;
  duration_secs: number;
  text: string | null;
  cached: boolean;
  error: string | null;
};

export type SearchSnippet = {
  time: number;
  sender: string | null;
  sender_name: string | null;
  text: string;
};

export type SearchHit = {
  username: string;
  display: string;
  is_group: boolean;
  hits: number;
  truncated: boolean;
  last_time: number | null;
  snippets: SearchSnippet[];
};

export type SearchReport = {
  keyword: string;
  scanned: number;
  skipped: number;
  hits: SearchHit[];
};

export type AgentCurrent = {
  configured: boolean;
  agent_id: string;
  agent_name: string;
  model?: string;
  command: string | null;
  available: boolean;
};

export type AgentSummary = {
  agent_id: string;
  agent_name: string;
  model: string;
  output: string;
  sessions: number;
  messages: number;
  characters: number;
  truncated: boolean;
  elapsed_ms: number;
};
