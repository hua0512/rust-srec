# Build stage
FROM node:26-alpine AS builder

WORKDIR /app

# Optional build identifier embedded into the UI bundle.
# Provide via `--build-arg VITE_UI_BUILD=...`.
ARG VITE_UI_BUILD
ENV VITE_UI_BUILD=$VITE_UI_BUILD

# Copy package files
COPY package.json pnpm-lock.yaml pnpm-workspace.yaml ./

# Install dependencies. node:26-alpine ships no corepack, so pnpm is installed
# directly; keep this version matching "packageManager" in package.json.
RUN npm install -g pnpm@11.11.0
RUN pnpm install --frozen-lockfile

# Copy source files
COPY . .

# Build the application
RUN pnpm run build

# Runtime stage
FROM node:26-alpine

WORKDIR /app

# nginx fronts the container, gettext supplies envsubst for the nginx
# template, tzdata backs the TZ variable, and su-exec lets the entrypoint drop
# the SSR server to an unprivileged account without a supervisor.
RUN apk add --no-cache nginx gettext tzdata su-exec

# Copy built assets. They stay root-owned and world-readable: the SSR server
# runs as `node` and only ever reads them, so it cannot rewrite its own code.
COPY --from=builder /app/.output ./.output

# Copy nginx configuration template
COPY nginx.conf.template /etc/nginx/templates/default.conf.template

# Copy entrypoint script
COPY entrypoint.sh /entrypoint.sh
RUN chmod +x /entrypoint.sh

# Default env vars
ENV PORT=3000
ENV HOST=0.0.0.0
ENV NODE_ENV=production
# Timezone (override at runtime via `-e TZ=...`)
ENV TZ=UTC
# Default backend if not set
ENV BACKEND_URL=http://rust-srec:8080

# Expose port 80 (Nginx)
EXPOSE 80

# Start the server via entrypoint
CMD ["/entrypoint.sh"]
