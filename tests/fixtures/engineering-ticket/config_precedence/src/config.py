DEFAULT_TIMEOUT = 30


def choose_timeout(environment, file_config):
    timeout = environment.get("APP_TIMEOUT", DEFAULT_TIMEOUT)
    if "timeout" in file_config:
        timeout = file_config["timeout"]
    return int(timeout)
