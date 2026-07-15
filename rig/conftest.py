# Self-test rig conftest — registers the "rig" pytest marker WITHOUT touching
# repo-level config (pytest.ini / pyproject.toml).  Keeps the 2 warnings clean.


def pytest_configure(config):
    config.addinivalue_line(
        "markers", "rig: self-test rig scenarios"
    )
