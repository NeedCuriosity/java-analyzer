//- /org/orm/Column.java lib:orm
package org.orm;

import java.lang.annotation.ElementType;
import java.lang.annotation.Retention;
import java.lang.annotation.RetentionPolicy;
import java.lang.annotation.Target;

@Retention(RetentionPolicy.RUNTIME)
@Target({ElementType.FIELD, ElementType.METHOD})
public @interface Column {}
//- /org/orm/Entity.java lib:orm
package org.orm;

import java.lang.annotation.ElementType;
import java.lang.annotation.Target;

@Target(ElementType.TYPE)
public @interface Entity {}
//- /org/orm/Param.java lib:orm
package org.orm;

import java.lang.annotation.ElementType;
import java.lang.annotation.Target;

@Target(ElementType.PARAMETER)
public @interface Param {}
//- /app/Model.java
package app;

import java.lang.annotation.ElementType;
import java.lang.annotation.Target;

import org.orm.Column;
import org.orm.Entity;
import org.orm.Param;

@Target(ElementType.LOCAL_VARIABLE)
@interface Temp {}

@/*@on_class*/
public class Model {
    @/*@on_field*/
    private int count;

    @Ov/*@on_method*/
    public String toString() { return ""; }

    void rename(@/*@on_param*/ String name) {
        @Te/*@on_local*/ int length = name.length();
    }
}
//? complete on_class has Entity Deprecated FunctionalInterface
//? complete on_class lacks Override Column Param SafeVarargs Temp
//? complete on_field order Column Entity
//? complete on_field order Deprecated Param
//? complete on_method first Override
//? complete on_param has Param Deprecated SuppressWarnings
//? complete on_param lacks Override Column Entity FunctionalInterface
//? complete on_local first Temp
//? stub org.orm.Column ; has "@java.lang.annotation.Target({java.lang.annotation.ElementType.FIELD, java.lang.annotation.ElementType.METHOD})"
