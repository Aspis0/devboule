"""Test file with Windows-style CRLF line endings.

This file uses CRLF line endings to verify that the chunking pipeline
correctly normalizes them before processing.
"""

class CrlfProcessor:
    """Process files that use Windows CRLF line endings."""

    def __init__(self, file_path):
        self.file_path = file_path
        self.lines = []

    def read_file(self):
        """Read and split the file on any line ending style."""
        with open(self.file_path, "rb") as f:
            raw = f.read()
        text = raw.decode("utf-8", errors="replace")
        text = text.replace("\r\n", "\n").replace("\r", "\n")
        self.lines = text.split("\n")
        return self.lines

    def count_definitions(self):
        """Count function and class definitions in the file."""
        import re
        count = 0
        for line in self.lines:
            stripped = line.strip()
            if re.match(r"^(def|class|fn|pub fn|async def)\s+\w+", stripped):
                count += 1
        return count

    def to_chunks(self, max_chars=2500):
        """Split the normalized text into overlapping chunks."""
        text = "\n".join(self.lines)
        chunks = []
        start = 0
        while start < len(text):
            end = min(len(text), start + max_chars)
            piece = text[start:end].strip()
            if piece:
                chunks.append({"start": start, "end": end, "text": piece})
            if end >= len(text):
                break
            start = max(0, end - 400)
        return chunks


def verify_crlf_handling():
    """Verify that CRLF files produce the same chunks as LF files."""
    processor = CrlfProcessor("test_crlf.txt")
    lines = processor.read_file()
    assert processor.count_definitions() >= 0
    chunks = processor.to_chunks()
    return len(chunks)
