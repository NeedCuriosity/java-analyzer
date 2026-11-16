//- /org/orm/Column.java lib:orm
package org.orm;

import java.lang.annotation.ElementType;
import java.lang.annotation.Retention;
import java.lang.annotation.RetentionPolicy;
import java.lang.annotation.Target;

@Retention(RetentionPolicy.RUNTIME)
@Target({ElementType.FIELD, ElementType.METHOD})
public @interface Column {}
//- /org/orm/Nullable.java lib:orm
package org.orm;

import java.lang.annotation.ElementType;
import java.lang.annotation.Target;

@Target(ElementType.TYPE_USE)
public @interface Nullable {}
//- /app/Model.java
package app;

import java.lang.annotation.ElementType;
import java.lang.annotation.Target;
import java.util.List;

import org.orm.Column;
import org.orm.Nullable;

@Target({ElementType.LOCAL_VARIABLE, ElementType.PARAMETER})
@interface Temp {}

@interface Anywhere {}

@/*@on_class*/Override
@Anywhere
public class Model<@Nullable T> {
    @Column @Nullable private String name;

    @/*@column_on_ctor*/Column
    public Model() {}

    @Column
    @Nullable
    public String getName() { return name; }

    @/*@type_use_on_void*/Nullable
    public void clear() {}

    @Override
    @Anywhere
    public String toString() { return ""; }

    void run(@Temp @Nullable String s, List<@Nullable String> names) {
        @Temp @Anywhere int length = s.length();
        try {
            names.add(s);
        } catch (@Temp RuntimeException e) {
            length++;
        }
        for (@Temp String n : names) {
            length += n.length();
        }
        @/*@temp_on_local_class*/Temp class Local {}
        new Local();
    }

    record Point(@Column @Nullable Integer x, @Temp int y) {}

    enum Kind { @Column ONE, @/*@temp_on_constant*/Temp TWO }
}
//? diag annotation-not-applicable on_class
//? diag annotation-not-applicable column_on_ctor
//? diag annotation-not-applicable type_use_on_void
//? diag annotation-not-applicable temp_on_local_class
//? diag annotation-not-applicable temp_on_constant
//? diag only annotation-not-applicable
