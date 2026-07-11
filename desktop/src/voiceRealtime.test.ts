import { describe, expect, it, vi } from "vitest";

import { handleRealtimeEvent } from "./voiceRealtime";

describe("Realtime voice event bridge", () => {
  it("uses the current output-audio transcript event", async () => {
    const onTranscript = vi.fn();
    await handleRealtimeEvent(
      JSON.stringify({
        type: "response.output_audio_transcript.done",
        transcript: "  All set.  "
      }),
      { onTranscript }
    );
    expect(onTranscript).toHaveBeenCalledWith("assistant", "All set.");
  });

  it("returns tool output to the model and continues exactly once", async () => {
    const onToolCall = vi.fn(async () => ({ action: "route_as_text", text: "fix it" }));
    const sent: Record<string, unknown>[] = [];
    const handled = new Set<string>();
    const event = {
      type: "response.function_call_arguments.done",
      call_id: "call_123",
      name: "route_request",
      arguments: JSON.stringify({ text: "fix it" })
    };
    const send = (value: Record<string, unknown>) => {
      sent.push(value);
      return true;
    };

    await handleRealtimeEvent(event, { onToolCall }, send, handled);
    await handleRealtimeEvent(event, { onToolCall }, send, handled);

    expect(onToolCall).toHaveBeenCalledTimes(1);
    expect(onToolCall).toHaveBeenCalledWith(
      "route_request",
      { text: "fix it" },
      "call_123"
    );
    expect(sent).toHaveLength(2);
    expect(sent[0]).toMatchObject({
      type: "conversation.item.create",
      item: { type: "function_call_output", call_id: "call_123" }
    });
    expect(JSON.parse(String((sent[0].item as Record<string, unknown>).output))).toEqual({
      action: "route_as_text",
      text: "fix it"
    });
    expect(sent[1]).toEqual({ type: "response.create" });
  });

  it("answers malformed tool arguments without invoking the router", async () => {
    const onToolCall = vi.fn();
    const sent: Record<string, unknown>[] = [];
    await handleRealtimeEvent(
      {
        type: "response.function_call_arguments.done",
        call_id: "call_bad",
        name: "route_request",
        arguments: "not-json"
      },
      { onToolCall },
      (value) => {
        sent.push(value);
        return true;
      }
    );
    expect(onToolCall).not.toHaveBeenCalled();
    expect(sent).toHaveLength(2);
    expect(String((sent[0].item as Record<string, unknown>).output)).toContain(
      "invalid tool arguments"
    );
  });
});
