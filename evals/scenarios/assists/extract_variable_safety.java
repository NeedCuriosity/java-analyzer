import java.util.List;

class ExtractCases {
    int collision(int a) { int extracted = 1; return /*@c1*/a + 2/*@c2*/ + extracted; }

    boolean guard(String s) { return s != null && /*@g1*/s.trim()/*@g2*/.isEmpty(); }

    boolean either(String s) { return s == null || /*@o1*/s.length()/*@o2*/ == 0; }

    int ternary(String s) { return s == null ? 0 : /*@t1*/s.length()/*@t2*/; }

    void loop(List<String> xs) {
        for (int i = 0; /*@l1*/i < 2/*@l2*/; i++) {}
        while (/*@w1*/xs.size()/*@w2*/ > 0) { xs.remove(0); }
        for (String x : /*@f1*/xs.subList(0, 1)/*@f2*/) { System.out.println(x); }
    }

    int branch(String s) {
        if (s.isEmpty()) return /*@b1*/s.length()/*@b2*/;
        int a = 1, b = /*@d1*/a + 1/*@d2*/;
        return b;
    }

    boolean left(String s) { return /*@k1*/s.isEmpty()/*@k2*/ && s.length() > 1; }

    String names(String name, String s) { return /*@n1*/s.trim()/*@n2*/ + /*@m1*/getName()/*@m2*/ + name; }

    String getName() { return "n"; }

    Class<?> kind(Object o) { return /*@q1*/o.getClass()/*@q2*/; }
}
//? assist extract_variable c1 c2 ; has "int extracted2 = a + 2;" ; has "return extracted2 + extracted;"
//? noassist extract_variable g1 g2
//? noassist extract_variable o1 o2
//? noassist extract_variable t1 t2
//? noassist extract_variable l1 l2
//? noassist extract_variable w1 w2
//? assist extract_variable f1 f2 ; has "List<String> subList = xs.subList(0, 1);" ; has "for (String x : subList)"
//? noassist extract_variable b1 b2
//? noassist extract_variable d1 d2
//? assist extract_variable k1 k2 ; has "boolean empty = s.isEmpty();" ; has "return empty && s.length() > 1;"
//? assist extract_variable n1 n2 ; has "String trim = s.trim();" ; has "return trim + getName() + name;"
//? assist extract_variable m1 m2 ; has "String name2 = getName();" ; has "return s.trim() + name2 + name;"
//? assist extract_variable q1 q2 ; has " extracted = o.getClass();" ; has "return extracted;"
