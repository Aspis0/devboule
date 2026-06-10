import { describe, it, expect } from "vitest";
import {
  replyKeyToBytes,
  replyTextToBytes,
  type ReplyKey,
} from "./replyKeyToBytes";

// The reply bar is the ONLY input path into the read-only terminal viewer, so the
// exact bytes each quick-key produces are the contract worth pinning. A wrong
// arrow final-byte (C/D swap) or a missing "\r" silently breaks prompts.

describe("replyKeyToBytes", () => {
  it("maps enter to a carriage return", () => {
    expect(replyKeyToBytes("enter")).toBe("\r");
  });

  it("maps yes/no to the letter plus a carriage return (answer + submit)", () => {
    expect(replyKeyToBytes("yes")).toBe("y\r");
    expect(replyKeyToBytes("no")).toBe("n\r");
  });

  it("maps arrow keys to the correct CSI cursor sequences (C=right, D=left)", () => {
    expect(replyKeyToBytes("up")).toBe("\x1b[A");
    expect(replyKeyToBytes("down")).toBe("\x1b[B");
    expect(replyKeyToBytes("left")).toBe("\x1b[D");
    expect(replyKeyToBytes("right")).toBe("\x1b[C");
  });

  it("maps esc to a bare ESC and ctrl-c to ETX (0x03)", () => {
    expect(replyKeyToBytes("esc")).toBe("\x1b");
    expect(replyKeyToBytes("ctrl-c")).toBe("\x03");
    // Spell out the byte values so a regression on the control codes is obvious.
    expect(replyKeyToBytes("esc").charCodeAt(0)).toBe(0x1b);
    expect(replyKeyToBytes("ctrl-c").charCodeAt(0)).toBe(0x03);
  });

  it("maps digits 1-4 to the digit plus a carriage return", () => {
    expect(replyKeyToBytes("1")).toBe("1\r");
    expect(replyKeyToBytes("2")).toBe("2\r");
    expect(replyKeyToBytes("3")).toBe("3\r");
    expect(replyKeyToBytes("4")).toBe("4\r");
  });

  it("covers every ReplyKey variant (no unmapped key falls through)", () => {
    const all: ReplyKey[] = [
      "enter",
      "yes",
      "no",
      "up",
      "down",
      "left",
      "right",
      "esc",
      "ctrl-c",
      "1",
      "2",
      "3",
      "4",
    ];
    for (const k of all) {
      expect(typeof replyKeyToBytes(k)).toBe("string");
      expect(replyKeyToBytes(k).length).toBeGreaterThan(0);
    }
  });
});

describe("replyTextToBytes", () => {
  it("appends a carriage return so the line is submitted", () => {
    expect(replyTextToBytes("hello")).toBe("hello\r");
  });

  it("sends a bare carriage return for empty text (accept default)", () => {
    expect(replyTextToBytes("")).toBe("\r");
  });

  it("preserves internal whitespace and special chars verbatim", () => {
    expect(replyTextToBytes("a b\tc")).toBe("a b\tc\r");
  });
});
