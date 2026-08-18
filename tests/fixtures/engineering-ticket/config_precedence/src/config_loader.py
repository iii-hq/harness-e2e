from .config import choose_timeout


def load_settings(environment, file_config):
    return {"timeout": choose_timeout(environment, file_config)}
