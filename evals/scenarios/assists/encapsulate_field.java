//- /app/Point.java
package app;

public class Point {
    public int /*@e1*/x;
    protected final String /*@e2*/label = "p";
    boolean /*@e3*/visible;
    private int /*@no1*/hidden;
    public static int /*@no2*/COUNT;

    int twice() {
        return x * 2;
    }
}

//- /app/Use.java
package app;

class Use {
    /** Writes {@link Point#x} directly. */
    int run(Point p) {
        p.x = 3;
        p.x += 2;
        p.x++;
        int y = p.x + 1;
        System.out.println(p.label + p.visible);
        p.visible = !p.visible;
        return y;
    }
}

//- /app/Bad.java
package app;

class Bad {
    int run(Point p) {
        return p.visible ? 1 : 0;
    }
}

//? assist encapsulate_field e1 ; has "Writes {@link Point#x} directly." ; has "    private int x;" ; has "        return x * 2;" ; has "    public int getX() {\n        return x;\n    }\n\n    public void setX(int x) {\n        this.x = x;\n    }" ; has "        p.setX(3);\n        p.setX(p.getX() + 2);\n        p.setX(p.getX() + 1);\n        int y = p.getX() + 1;"
//? assist encapsulate_field e2 ; has "    private final String label = \"p\";" ; has "public String getLabel() {" ; lacks "setLabel" ; has "System.out.println(p.getLabel() + p.visible);"
//? assist encapsulate_field e3 ; has "    private boolean visible;" ; has "public boolean isVisible() {" ; has "p.setVisible(!p.isVisible());" ; has "return p.isVisible() ? 1 : 0;"
//? noassist encapsulate_field no1
//? noassist encapsulate_field no2
