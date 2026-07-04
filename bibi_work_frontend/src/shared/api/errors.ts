export class ApiError extends Error {
  readonly status: number;
  readonly code?: string;
  readonly details?: unknown;

  constructor(message: string, status: number, code?: string, details?: unknown) {
    super(message);
    this.name = "ApiError";
    this.status = status;
    this.code = code;
    this.details = details;
  }
}

export class AuthExpiredError extends ApiError {
  constructor(message = "Authentication token is missing or expired") {
    super(message, 401, "AUTH_EXPIRED");
    this.name = "AuthExpiredError";
  }
}

export class ForbiddenError extends ApiError {
  constructor(message = "The current user is not allowed to perform this action") {
    super(message, 403, "FORBIDDEN");
    this.name = "ForbiddenError";
  }
}

export class ContractError extends Error {
  readonly details: unknown;

  constructor(message: string, details: unknown) {
    super(message);
    this.name = "ContractError";
    this.details = details;
  }
}
