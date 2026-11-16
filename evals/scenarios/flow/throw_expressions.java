import java.io.IOException;
import java.util.function.Supplier;

class Main {
    static class AEx extends Exception {}

    static IOException wrapIo(Throwable cause) {
        return new IOException(cause);
    }

    static RuntimeException wrapRuntime(Throwable cause) {
        return new IllegalStateException(cause);
    }

    static <X extends Throwable> X generic(Throwable cause) {
        return null;
    }

    void call(Throwable cause) {
        throw /*@call*/wrapIo(cause);
    }

    void cast(Throwable cause) {
        throw /*@cast*/(AEx) cause;
    }

    void supplied(Supplier<IOException> supplier) {
        throw /*@supplied*/supplier.get();
    }

    void neverThrown() {
        try {
            throw wrapRuntime(null);
        } catch (/*@never*/AEx e) {
            System.out.println(e);
        }
    }

    void ok1(Throwable cause) {
        throw wrapRuntime(cause);
    }

    void ok2(Throwable cause) throws IOException {
        throw wrapIo(cause);
    }

    void ok3(Throwable cause) throws AEx {
        throw (AEx) cause;
    }

    void ok4(Throwable cause) {
        throw (RuntimeException) cause;
    }

    void ok5(Throwable cause) {
        try {
            throw wrapIo(cause);
        } catch (IOException e) {
            System.out.println(e);
        }
    }

    void ok6(Supplier<? extends RuntimeException> supplier) {
        throw supplier.get();
    }
}
//? diag unhandled-exception call
//? diag unhandled-exception cast
//? diag unhandled-exception supplied
//? diag exception-never-thrown never
//? diag exact
