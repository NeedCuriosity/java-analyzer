import java.io.FileNotFoundException;
import java.io.IOException;
import java.io.Reader;
import java.io.StringReader;
import java.util.List;

class Main {
    static class AEx extends Exception {}

    void io() throws IOException {}

    void any() throws Exception {}

    void a() throws AEx {}

    <X extends Exception> void generic(Class<X> type) throws X {}

    void never() {
        try {
            System.out.println("x");
        } catch (/*@never*/IOException e) {
            System.out.println(e);
        }
    }

    void neverMulti() {
        try {
            a();
        } catch (AEx | /*@multi*/IOException e) {
            System.out.println(e);
        }
    }

    void afterOtherCatch() {
        try {
            a();
        } catch (AEx e) {
            System.out.println(e);
        } catch (/*@sub*/FileNotFoundException e) {
            System.out.println(e);
        }
    }

    void ok1() {
        try {
            io();
        } catch (FileNotFoundException e) {
            System.out.println(e);
        } catch (IOException e) {
            System.out.println(e);
        }
    }

    void ok2() {
        try {
            any();
        } catch (IOException e) {
            System.out.println(e);
        } catch (Exception e) {
            System.out.println(e);
        }
    }

    void ok3() {
        try {
            System.out.println();
        } catch (Exception e) {
            System.out.println(e);
        }
    }

    void ok4() {
        try {
            System.out.println();
        } catch (Throwable e) {
            System.out.println(e);
        }
    }

    void ok5() {
        try {
            System.out.println();
        } catch (RuntimeException e) {
            System.out.println(e);
        }
    }

    void ok6() {
        try (Reader r = new StringReader("x")) {
            System.out.println(r);
        } catch (IOException e) {
            System.out.println(e);
        }
    }

    void ok7(List<String> names) {
        try {
            names.forEach(n -> System.out.println(n));
            generic(IOException.class);
        } catch (IOException e) {
            System.out.println(e);
        }
    }

    void ok8() {
        try {
            throw new AEx();
        } catch (AEx e) {
            System.out.println(e);
        }
    }

    void ok9() {
        try {
            a();
        } catch (AEx e) {
            System.out.println(e);
        } catch (Exception e) {
            System.out.println(e);
        }
    }

    void ok10() {
        try {
            try {
                io();
            } finally {
                System.out.println();
            }
        } catch (IOException e) {
            System.out.println(e);
        }
    }
}
//? diag exception-never-thrown never
//? diag exception-never-thrown multi
//? diag exception-never-thrown sub
//? diag exact
