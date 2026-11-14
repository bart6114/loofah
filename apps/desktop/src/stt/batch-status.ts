export class IncompleteTranscriptionError extends Error {
  constructor() {
    super(
      "Incomplete transcript. Your recovered text is saved. Retry missing audio to finish.",
    );
    this.name = "IncompleteTranscriptionError";
  }
}
