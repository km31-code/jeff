/**
 * Jeff v1 provider contracts (Phase 0 only).
 *
 * These are architecture-level interfaces only.
 * No concrete provider implementations are allowed in Phase 0.
 */

export interface SpeechToTextProvider {
  transcribe(input: { audioBytes: Uint8Array; mimeType: string }): Promise<{ text: string }>;
}

export interface TextToSpeechProvider {
  synthesize(input: { text: string; voice?: string }): Promise<{ audioBytes: Uint8Array; mimeType: string }>;
}

export interface ReasoningModelProvider {
  generate(input: {
    prompt: string;
    systemPrompt?: string;
    maxTokens?: number;
  }): Promise<{ text: string }>;
}

export interface EmbeddingsProvider {
  embed(input: { text: string }): Promise<{ vector: number[] }>;
}
