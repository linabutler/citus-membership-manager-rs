# Citus Membership Manager for Docker

In a [Dockerized Citus cluster](https://github.com/citusdata/docker), the membership manager keeps the master up-to-date with all the workers in the cluster. This is a Rust port of the original [Citus membership manager](https://github.com/citusdata/membership-manager), updated for compatibility with newer Docker versions.

## Quick Start

### Using Docker Compose

```yaml
version: "3.8"
services:
  master:
    image: citusdata/citus:latest
    labels:
      - "com.citusdata.role=Master"
    environment:
      POSTGRES_DB: db
      POSTGRES_USER: postgres
      POSTGRES_PASSWORD: postgres
    # ...

  manager:
    depends_on:
      - master
    image: linabutler/citus-membership-manager-rs:latest
    volumes:
      - /var/run/docker.sock:/var/run/docker.sock:ro
    environment:
      CITUS_HOST: master
      POSTGRES_DB: db
      POSTGRES_USER: postgres
      POSTGRES_PASSWORD: postgres
    volumes:
      - citus-healthcheck:/healthcheck


  worker:
    depends_on:
      - manager
    image: citusdata/citus:latest
    labels:
      - "com.citusdata.role=Worker"
    environment:
      POSTGRES_DB: db
      POSTGRES_USER: postgres
      POSTGRES_PASSWORD: postgres
    volumes:
      - citus-healthcheck:/healthcheck
    command: "/wait-for-manager.sh"
    # ...

volumes:
  citus-healthcheck:
```

### Docker Build

```bash
docker build -t citus-membership-manager-rs .

docker run -v /var/run/docker.sock:/var/run/docker.sock:ro \
  -e CITUS_HOST=master \
  -e POSTGRES_DB=db \
  -e POSTGRES_USER=postgres \
  -e POSTGRES_PASSWORD=postgres \
  citus-membership-manager-rs
```

## How It Works

Configure the manager using environment variables:

| Variable | Description | Default |
|----------|-------------|---------|
| `CITUS_HOST` | Hostname of the Citus master node | `master` |
| `POSTGRES_USER` | PostgreSQL user | `postgres` |
| `POSTGRES_PASSWORD` | PostgreSQL password | (none) |
| `POSTGRES_DB` | Database name | Same as `POSTGRES_USER` |
| `HOSTNAME` | Current container's hostname | (required) |

The manager inspects its own container to determine the Docker Compose project that it belongs to, and then subscribes to Docker events for worker containers within the same project. When a worker reports that it's healthy, the manager tells the master to add that worker to the cluster. When a worker container is deleted, the manager tells the master to forget that worker.

## License

```
Copyright © 2025 Lina Butler
Copyright © 2017 Citus Data, Inc.
```

Licensed under the Apache License, Version 2.0 (the “License”); you may not use this file except in compliance with the License. You may obtain a copy of the License at http://www.apache.org/licenses/LICENSE-2.0

Unless required by applicable law or agreed to in writing, software distributed under the License is distributed on an “AS IS” BASIS, WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied. See the License for the specific language governing permissions and limitations under the License.
