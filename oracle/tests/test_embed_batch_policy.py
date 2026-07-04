"""Hardware-adaptive embed batch sizing: the encode batch must scale UP with
the machine (CUDA VRAM / unified RAM headroom) instead of the old flat
worst-case default (4), while every existing back-off guard (OOM->CPU, RAM
floor, thermal cooldown) stays in charge of scaling DOWN."""

import oracle.ingestion.embedder as embedder
from oracle.ingestion.embedder import choose_embed_batch_size


class TestOverride:
    def test_explicit_override_wins_over_everything(self):
        assert choose_embed_batch_size("cuda", 64.0, 24.0, "4") == 4

    def test_garbage_override_falls_through_to_policy(self):
        assert choose_embed_batch_size("cpu", 64.0, None, "abc") == 8

    def test_non_positive_override_falls_through(self):
        assert choose_embed_batch_size("cpu", 64.0, None, "0") == 8
        assert choose_embed_batch_size("cpu", 64.0, None, "-1") == 8


class TestCuda:
    def test_big_vram_goes_max(self):
        assert choose_embed_batch_size("cuda", 8.0, 12.0, "") == 64

    def test_mid_vram(self):
        assert choose_embed_batch_size("cuda", 8.0, 5.0, "") == 32

    def test_small_vram(self):
        assert choose_embed_batch_size("cuda", 8.0, 3.0, "") == 16

    def test_unknown_vram_is_conservative(self):
        assert choose_embed_batch_size("cuda", 8.0, None, "") == 16


class TestMps:
    def test_big_unified_ram(self):
        assert choose_embed_batch_size("mps", 40.0, None, "") == 32

    def test_mid_ram(self):
        assert choose_embed_batch_size("mps", 10.0, None, "") == 16

    def test_tight_ram_keeps_old_floor(self):
        assert choose_embed_batch_size("mps", 5.0, None, "") == 8

    def test_unknown_ram_is_conservative(self):
        assert choose_embed_batch_size("mps", None, None, "") == 8


class TestCpuAndUnknownDevice:
    def test_cpu_with_headroom(self):
        assert choose_embed_batch_size("cpu", 64.0, None, "") == 8

    def test_cpu_tight_ram_keeps_legacy_batch(self):
        assert choose_embed_batch_size("cpu", 4.0, None, "") == 4

    def test_cpu_unknown_ram_is_conservative(self):
        assert choose_embed_batch_size("cpu", None, None, "") == 4

    def test_unknown_device_follows_cpu_policy(self):
        assert choose_embed_batch_size(None, 64.0, None, "") == 8
        assert choose_embed_batch_size(None, None, None, "") == 4


class TestEffectiveChunkBatchSize:
    def test_explicit_value_wins_even_when_equal_to_default(self, monkeypatch):
        # An operator pinning --batch-chunks 8 must get literally 8 even though
        # 8 is also the config default (tri-state: explicit int is never
        # second-guessed).
        from oracle.config import CHUNK_BATCH_CHUNKS
        from oracle.ingestion.chunk_index import effective_chunk_batch_size

        monkeypatch.delenv("ORACLE_CHUNK_BATCH_CHUNKS", raising=False)
        assert effective_chunk_batch_size(CHUNK_BATCH_CHUNKS) == CHUNK_BATCH_CHUNKS
        assert effective_chunk_batch_size(CHUNK_BATCH_CHUNKS + 3) == CHUNK_BATCH_CHUNKS + 3

    def test_env_override_applies_when_nothing_passed(self, monkeypatch):
        import oracle.ingestion.chunk_index as chunk_index

        monkeypatch.setenv("ORACLE_CHUNK_BATCH_CHUNKS", "whatever-nonempty")
        monkeypatch.setattr(
            chunk_index,
            "effective_embed_batch_size",
            lambda: (_ for _ in ()).throw(AssertionError("must not be consulted")),
        )
        from oracle.config import CHUNK_BATCH_CHUNKS

        assert chunk_index.effective_chunk_batch_size(None) == max(1, CHUNK_BATCH_CHUNKS)

    def test_none_scales_with_embed_batch(self, monkeypatch):
        import oracle.ingestion.chunk_index as chunk_index

        monkeypatch.delenv("ORACLE_CHUNK_BATCH_CHUNKS", raising=False)
        monkeypatch.setattr(chunk_index, "effective_embed_batch_size", lambda: 32)
        assert chunk_index.effective_chunk_batch_size(None) == 128

    def test_none_never_below_the_configured_default(self, monkeypatch):
        import oracle.ingestion.chunk_index as chunk_index
        from oracle.config import CHUNK_BATCH_CHUNKS

        monkeypatch.delenv("ORACLE_CHUNK_BATCH_CHUNKS", raising=False)
        monkeypatch.setattr(chunk_index, "effective_embed_batch_size", lambda: 1)
        assert chunk_index.effective_chunk_batch_size(None) == CHUNK_BATCH_CHUNKS


class TestEffectiveBatchIntegration:
    def test_env_override_respected(self, monkeypatch):
        monkeypatch.setenv("ORACLE_EMBED_BATCH_SIZE", "12")
        assert embedder.effective_embed_batch_size() == 12

    def test_no_env_returns_positive_policy_value(self, monkeypatch):
        monkeypatch.delenv("ORACLE_EMBED_BATCH_SIZE", raising=False)
        value = embedder.effective_embed_batch_size()
        assert isinstance(value, int) and value >= 8
