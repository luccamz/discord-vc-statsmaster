# Discord VC Stats Master

## Prerequisites

* Rust toolchain
* A Discord Bot Token
* SQLx CLI

## Local Setup

1. Clone the repository: `git clone https://github.com/luccamz/discord-vc-statsmaster`
2. Navigate into the directory: `cd discord-vc-statsmaster`
3. Install the SQLx CLI tool (if not already installed): `cargo install sqlx-cli --no-default-features --features rustls,sqlite`
4. Copy the environment template: `cp .env.example .env`
5. Insert your Discord bot token and define the database URL in the `.env` file:

```text
DISCORD_TOKEN=your_bot_token_here
DATABASE_URL=sqlite://stats.db
```

6. Initialize the database and apply all migrations to prepare the schema for compile-time validation: `sqlx database setup`
7. Compile and run the application: `cargo run --release`
