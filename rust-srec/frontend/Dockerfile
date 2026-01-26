# Build stage
FROM node:22-alpine AS builder

WORKDIR /app

# Optional build identifier embedded into the UI bundle.
# Provide via `--build-arg VITE_UI_BUILD=...`.
ARG VITE_UI_BUILD
ENV VITE_UI_BUILD=$VITE_UI_BUILD

# Copy package files
COPY package.json pnpm-lock.yaml pnpm-workspace.yaml ./

# Install dependencies
RUN corepack enable
RUN corepack prepare pnpm@10.28.1 --activate
RUN pnpm install --frozen-lockfile

# Copy source files
COPY . .

# Build the application
RUN pnpm run build

# Runtime stage
FROM node:22-alpine

WORKDIR /app

# Install Nginx and gettext (for envsubst)
RUN apk add --no-cache nginx gettext tzdata

# Copy built assets
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
