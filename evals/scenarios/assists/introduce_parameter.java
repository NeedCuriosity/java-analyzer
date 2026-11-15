import java.util.List;

class Main {
    static final int LIMIT = 10;

    private static String greet(String name) {
        return /*@g1*/"Hello"/*@g2*/ + ", " + name;
    }

    private static int scaled(int value, int factor) {
        return value * /*@s1*/Math.max(factor + 1, LIMIT)/*@s2*/;
    }

    private int total(List<String> items) {
        int sum = 0;
        for (String item : items) {
            sum += /*@l1*/item.length()/*@l2*/;
        }
        return sum + /*@sz1*/items.size()/*@sz2*/;
    }

    private static int reassigned(int n) {
        n++;
        return /*@r1*/n * 2/*@r2*/;
    }

    void calls(int k, List<String> names) {
        greet("Ann");
        scaled(3, k + 2);
        total(names);
        reassigned(1);
    }
}
//? assist introduce_parameter g1 g2 ; has "private static String greet(String name, String text) {" ; has "return text + \", \" + name;" ; has "greet(\"Ann\", \"Hello\");"
//? assist introduce_parameter s1 s2 ; has "private static int scaled(int value, int factor, int max) {" ; has "return value * max;" ; has "scaled(3, k + 2, Math.max((k + 2) + 1, LIMIT));"
//? assist introduce_parameter sz1 sz2 ; has "private int total(List<String> items, int size) {" ; has "return sum + size;" ; has "total(names, names.size());"
//? noassist introduce_parameter l1 l2
//? noassist introduce_parameter r1 r2
