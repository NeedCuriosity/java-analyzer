import java.io.IOException;
import java.io.InputStream;
import java.util.List;
import java.util.Set;

class Main {
    static final int SMALL = 10;
    static final char LETTER = 'a';
    int counter;

    static void log() {}

    void operators(int count, Integer boxed, String s, boolean flag, Boolean wrapped, double d, Object o) {
        boolean a = /*@o1*/count == null;
        boolean b = /*@o2*/1 == "a";
        int c = /*@o3*/1 << 1.0;
        int e = /*@o4*/(int) d & 2.0;
        String f = /*@o5*/"" + log();
        boolean ok1 = boxed == null && s == null && o == null;
        boolean ok2 = count == boxed && flag == wrapped;
        int ok3 = count << 2 | count & 0xFF;
        boolean ok4 = flag & wrapped;
        System.out.println("" + a + b + c + e + f + ok1 + ok2 + ok3 + ok4);
    }

    void compound(int count, boolean flag, String s, Object o, char c, long l, Integer boxed) {
        /*@k1*/flag += 1;
        /*@k2*/count += "x";
        /*@k3*/count &= true;
        s += 1;
        o += "x";
        count += 1.5;
        c += 1;
        l <<= 2;
        flag &= true;
        boxed += 1;
        System.out.println("" + flag + count + s + o + c + l + boxed);
    }

    void conditionals(List<String> names, boolean flag, int n) {
        Integer a = flag ? /*@q1*/names : null;
        String b = /*@q2*/flag ? 1 : 2;
        int c = /*@q3*/flag ? 1 : 2L;
        String d = flag ? "x" : /*@q4*/n;
        long ok1 = flag ? 1 : 2L;
        Integer ok2 = flag ? 1 : null;
        Object ok3 = flag ? 1 : "x";
        Character ok4 = flag ? 'a' : 0;
        byte ok5 = flag ? 1 : 2;
        double ok6 = flag ? 1 : 2.0f;
        System.out.println("" + a + b + c + d + ok1 + ok2 + ok3 + ok4 + ok5 + ok6);
    }

    byte narrowing(int n, char ch, short sh, InputStream in) throws IOException {
        byte a = /*@n1*/200;
        char b = /*@n2*/-1;
        byte c = /*@n3*/n + 1;
        char d = /*@n4*/ch + 1;
        short e = /*@n5*/counter;
        byte f = /*@n6*/in.read();
        byte g = /*@n7*/sh;
        final int fixed = 3;
        byte ok1 = 10;
        byte ok2 = -128;
        char ok3 = 'a' + 1;
        short ok4 = SMALL;
        byte ok5 = fixed + 1;
        char ok6 = LETTER + 1;
        byte ok7 = 0xFFFFFFFF;
        Byte ok8 = 1;
        byte ok9 = (byte) n;
        System.out.println("" + a + b + c + d + e + f + g + ok1 + ok2 + ok3 + ok4 + ok5 + ok6 + ok7 + ok8 + ok9);
        return /*@n8*/n;
    }

    <T> T generic(T input, int n, Object o) {
        T a = /*@g1*/n;
        T ok1 = input;
        T ok2 = null;
        T ok3 = (T) o;
        System.out.println("" + a + ok1 + ok2 + ok3);
        if (n > 0) {
            return /*@g2*/"x";
        }
        return input;
    }

    void calls(List<String> names, Set<String> others) {
        names./*@m1*/addAll("x");
        names./*@m2*/containsAll(1);
        names.addAll(others);
        names.addAll(0, others);
        names.containsAll(others);
    }
}
//? diag bad-operand o1
//? diag bad-operand o2
//? diag bad-operand o3
//? diag bad-operand o4
//? diag bad-operand o5
//? diag bad-operand k1
//? diag bad-operand k2
//? diag bad-operand k3
//? diag type-mismatch q1
//? diag type-mismatch q2
//? diag type-mismatch q3
//? diag type-mismatch q4
//? diag type-mismatch n1
//? diag type-mismatch n2
//? diag type-mismatch n3
//? diag type-mismatch n4
//? diag type-mismatch n5
//? diag type-mismatch n6
//? diag type-mismatch n7
//? diag type-mismatch n8
//? diag type-mismatch g1
//? diag type-mismatch g2
//? diag argument-mismatch m1
//? diag argument-mismatch m2
//? diag exact
