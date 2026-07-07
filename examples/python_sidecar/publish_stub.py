from drission_sidecar import asset, failed, profile_dir, succeeded


def main():
    current = asset() or {}

    # Replace this branch with the existing Python browser/business logic.
    if current.get("label") == "rate-limited-demo":
        failed(
            "rate_limited",
            "platform returned rate limit",
            cooldown_seconds=900,
            next_state="repair",
        )
        raise SystemExit(1)

    succeeded(
        "published",
        result={
            "accountId": current.get("accountId"),
            "profileDir": profile_dir(),
        },
    )


if __name__ == "__main__":
    main()
