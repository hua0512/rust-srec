export class BackendApiError extends Error {
  constructor(
    public status: number,
    public statusText: string,
    public body: any,
  ) {
    // Extract detailed message from body if available
    const detail =
      typeof body === 'object' && body !== null
        ? body.message || body.detail || body.error || JSON.stringify(body)
        : typeof body === 'string' && body.length > 0
          ? body
          : `${status} ${statusText}`;
    super(detail);
    this.name = 'BackendApiError';
  }
}
