package org.finos.legend.pure.rust;

/**
 * Exception thrown when an error occurs during Pure evaluation in the Rust engine.
 */
public class PureRustEvaluationException extends PureRustException
{
    // todo kind, stack trace?
    public PureRustEvaluationException(String message)
    {
        super(message);
    }
}
