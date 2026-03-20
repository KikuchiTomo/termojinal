/** Message sent from command script to jterm (via stdout) */
export type CommandMessage =
  | FuzzyMessage
  | MultiMessage
  | ConfirmMessage
  | TextMessage
  | InfoMessage
  | DoneMessage
  | ErrorMessage;

export interface FuzzyMessage {
  type: "fuzzy";
  prompt: string;
  items: FuzzyItem[];
  preview?: boolean;
}

export interface MultiMessage {
  type: "multi";
  prompt: string;
  items: FuzzyItem[];
}

export interface ConfirmMessage {
  type: "confirm";
  message: string;
  default?: boolean;
}

export interface TextMessage {
  type: "text";
  label: string;
  placeholder?: string;
  default?: string;
  completions?: string[];
}

export interface InfoMessage {
  type: "info";
  message: string;
}

export interface DoneMessage {
  type: "done";
  notify?: string;
}

export interface ErrorMessage {
  type: "error";
  message: string;
}

export interface FuzzyItem {
  value: string;
  label?: string;
  description?: string;
  preview?: string;
  icon?: string;
}

/** Response sent from jterm to command script (via stdin) */
export type CommandResponse =
  | SelectedResponse
  | MultiSelectedResponse
  | ConfirmedResponse
  | TextInputResponse
  | CancelledResponse;

export interface SelectedResponse {
  type: "selected";
  value: string;
}

export interface MultiSelectedResponse {
  type: "multi_selected";
  values: string[];
}

export interface ConfirmedResponse {
  type: "confirmed";
  yes: boolean;
}

export interface TextInputResponse {
  type: "text_input";
  value: string;
}

export interface CancelledResponse {
  type: "cancelled";
}
