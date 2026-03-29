# Discord VC StatsMaster

This is a very simple Discord bot that tracks time spent on certain voice channels, manages user-specific tasks with deadlines in the form of a todo list, and automates weekly server statistics reporting and resetting. The original intended purpose of this bot is to track the time one spends studying and which topics.

The time leaderboard and personal record tracking is meant to serve as extra motivation and stimulate a little bit of competition between users, as I built this for my and my friends' personal use.

## Features

* **Voice Channel Tracking**: Monitors and logs the duration users spend in designated voice channels.
* **Task Management**: Allows users to create tasks and set relative or absolute deadlines using slash commands (`/add_task`, `/edit_deadline`).
* **Automated Reporting**: Executes weekly statistics resets and publishes summaries or changelogs to configured text channels.

## Discord Application Setup

Create an application in the Discord Developer Portal and configure the following parameters to generate your OAuth2 invite URL.

### OAuth2 Scopes

| Scope | Purpose |
|---|---|
| `bot` | Enables the bot to join the guild and perform standard actions. |
| `applications.commands` | Allows the bot to register and process slash commands. |

### Bot Permissions

| Permission | Purpose |
|---|---|
| View Channels | Allows the bot to establish presence and detect voice or text channels. |
| Send Messages | Required to send standard command responses and system alerts. |
| Embed Links | Required to render interactive setup dashboards and formatted task lists. |

## Local Setup

1. Clone the repository: `git clone https://github.com/luccamz/discord-vc-statsmaster`
2. Navigate into the directory: `cd discord-vc-statsmaster`
3. Install the SQLx CLI tool: `cargo install sqlx-cli --no-default-features --features rustls,sqlite`
4. Copy the environment template: `cp .env.example .env`
5. Define your environment variables in the `.env` file:

```text
DISCORD_TOKEN=your_bot_token_here
DATABASE_URL=sqlite://stats.db
```

6. Initialize the database and apply schema migrations: `sqlx database setup`
7. Compile and run the application: `cargo run --release`
