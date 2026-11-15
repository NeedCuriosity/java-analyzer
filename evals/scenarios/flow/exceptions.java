import java.io.FileReader;
import java.io.IOException;
import java.io.Reader;
import java.util.List;
import java.util.concurrent.Callable;

class Main {
    void thrower() throws IOException {}
    void custom() throws Exception {}

    void unhandled() {
        /*@call*/thrower();
    }

    void unhandledThrow() {
        throw new /*@throw_stmt*/IOException("x");
    }

    void unhandledCtor() {
        Reader r = new /*@ctor*/FileReader("f");
    }

    void wrongCatch() {
        try {
            /*@wrong_catch*/custom();
        } catch (IOException e) {
        }
    }

    void inLambda(List<String> names) {
        names.forEach(n -> /*@in_lambda*/thrower());
    }

    void unhandledClose() {
        try (/*@close*/Reader r = null) {
        }
    }

    void ok1() throws IOException { thrower(); }

    void ok2() {
        try {
            thrower();
        } catch (IOException e) {
            throw new RuntimeException(e);
        }
    }

    void ok3() {
        try {
            custom();
        } catch (RuntimeException e) {
            throw e;
        } catch (Exception e) {
        }
    }

    void ok4() throws Exception {
        Callable<Integer> c = () -> { thrower(); return 1; };
        c.call();
    }

    void ok5() {
        try (Reader r = new FileReader("f")) {
            r.read();
        } catch (IOException | IllegalStateException e) {
        }
    }

    void ok6() {
        throw new IllegalArgumentException();
    }

    void ok7() throws Throwable {
        try {
            custom();
        } catch (Throwable t) {
            throw t;
        }
    }
}
//? diag unhandled-exception call
//? diag unhandled-exception throw_stmt
//? diag unhandled-exception ctor
//? diag unhandled-exception wrong_catch
//? diag unhandled-exception in_lambda
//? diag unhandled-exception close
//? diag exact
