import java.io.Serializable;
import java.util.ArrayList;
import java.util.List;

class Main {
    interface Shape {}
    static class Square implements Shape {}
    static class Open {}
    static final class Closed {}

    static void log() {}

    void dereference(int count, String s, Integer boxed, int[] values) {
        count./*@d1*/toString();
        int len = count./*@d2*/length;
        log()./*@d3*/toString();
        int ok1 = s.length() + boxed.intValue() + values.length + values.clone().length;
        System.out.println(len + ok1);
    }

    void casts(Object o, Number n, CharSequence cs, List<String> names, Long big, Shape shape, Open open) {
        String a = (String) /*@c1*/1;
        Integer b = (Integer) /*@c2*/"x";
        int c = (int) /*@c3*/big;
        boolean d = (boolean) /*@c4*/1;
        Runnable e = (Runnable) /*@c5*/"x";
        Closed f = (Closed) /*@c6*/shape;
        String g = (String) /*@c7*/names;
        int ok1 = (int) o;
        long ok2 = (long) n;
        String ok3 = (String) cs;
        ArrayList<String> ok4 = (ArrayList<String>) names;
        Runnable ok5 = (Runnable) open;
        Square ok6 = (Square) shape;
        Number ok7 = (Number) 1;
        Serializable ok8 = (Serializable) 1;
        char ok9 = (char) 65;
        long ok10 = (long) Integer.valueOf(3);
        System.out.println("" + a + b + c + d + e + f + g + ok1 + ok2 + ok3 + ok4 + ok5 + ok6 + ok7 + ok8 + ok9 + ok10);
    }

    boolean instances(String s, Object o, Shape shape, Open open, List<String> names) {
        return /*@i1*/s instanceof Integer
            || /*@i2*/names instanceof String
            || o instanceof String
            || shape instanceof Square
            || shape instanceof Open
            || open instanceof Runnable
            || names instanceof ArrayList;
    }

    void throwing(int code, Exception cause) throws Exception {
        if (code == 1) throw /*@t1*/new Object();
        if (code == 2) throw /*@t2*/"failed";
        if (code == 3) throw cause;
        throw new IllegalStateException();
    }
}
//? diag unresolved-method d1
//? diag unresolved-field d2
//? diag unresolved-method d3
//? diag type-mismatch c1
//? diag type-mismatch c2
//? diag type-mismatch c3
//? diag type-mismatch c4
//? diag type-mismatch c5
//? diag type-mismatch c6
//? diag type-mismatch c7
//? diag type-mismatch i1
//? diag type-mismatch i2
//? diag type-mismatch t1
//? diag type-mismatch t2
//? diag exact
