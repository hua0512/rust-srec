# Build stage
FROM node:22-alpine AS builder

WORKDIR /app

# Copy package files
COPY package.json package-lock.json ./

# Install dependencies
RUN npm ci

# Copy source files
COPY . .

# Build the application
RUN npm run build

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
# Default backend if not set
ENV BACKEND_URL=http://rust-srec:8080

# Expose port 80 (Nginx)
EXPOSE 80

# Start the server via entrypoint
CMD ["/entrypoint.sh"]
